//! A small in-memory filesystem: inodes in a table, directories as sorted
//! maps, files as byte vectors. It also stores the checkpoint images, so
//! `ls /ckpt` shows exactly what the ghost engine has saved.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

pub type Ino = usize;
pub const ROOT: Ino = 0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    File,
    Dir,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FsError {
    NotFound,
    NotDir,
    IsDir,
    Exists,
    BadPath,
    NotEmpty,
}

impl FsError {
    pub fn errno(self) -> i64 {
        match self {
            FsError::NotFound => -2,
            FsError::NotDir => -20,
            FsError::IsDir => -21,
            FsError::Exists => -17,
            FsError::BadPath => -22,
            FsError::NotEmpty => -39,
        }
    }
}

pub struct Inode {
    pub kind: Kind,
    pub data: Vec<u8>,
    pub children: BTreeMap<String, Ino>,
    pub parent: Ino,
    pub name: String,
}

pub struct RamFs {
    inodes: Vec<Option<Inode>>,
}

pub static FS: Mutex<RamFs> = Mutex::new(RamFs { inodes: Vec::new() });

impl RamFs {
    pub fn format(&mut self) {
        self.inodes.clear();
        self.inodes.push(Some(Inode {
            kind: Kind::Dir,
            data: Vec::new(),
            children: BTreeMap::new(),
            parent: ROOT,
            name: String::new(),
        }));
    }

    pub fn inode(&self, ino: Ino) -> Option<&Inode> {
        self.inodes.get(ino).and_then(|i| i.as_ref())
    }

    fn inode_mut(&mut self, ino: Ino) -> Option<&mut Inode> {
        self.inodes.get_mut(ino).and_then(|i| i.as_mut())
    }

    pub fn lookup(&self, path: &str) -> Result<Ino, FsError> {
        if !path.starts_with('/') {
            return Err(FsError::BadPath);
        }
        let mut cur = ROOT;
        for part in path.split('/').filter(|p| !p.is_empty()) {
            let node = self.inode(cur).ok_or(FsError::NotFound)?;
            if node.kind != Kind::Dir {
                return Err(FsError::NotDir);
            }
            cur = match part {
                "." => cur,
                ".." => node.parent,
                _ => *node.children.get(part).ok_or(FsError::NotFound)?,
            };
        }
        Ok(cur)
    }

    fn split(path: &str) -> Result<(&str, &str), FsError> {
        let path = path.trim_end_matches('/');
        let i = path.rfind('/').ok_or(FsError::BadPath)?;
        let (dir, name) = (&path[..i], &path[i + 1..]);
        if name.is_empty() || name == "." || name == ".." {
            return Err(FsError::BadPath);
        }
        Ok((if dir.is_empty() { "/" } else { dir }, name))
    }

    pub fn create(&mut self, path: &str, kind: Kind) -> Result<Ino, FsError> {
        let (dir, name) = Self::split(path)?;
        let parent = self.lookup(dir)?;
        let p = self.inode(parent).ok_or(FsError::NotFound)?;
        if p.kind != Kind::Dir {
            return Err(FsError::NotDir);
        }
        if p.children.contains_key(name) {
            return Err(FsError::Exists);
        }
        let node = Inode {
            kind,
            data: Vec::new(),
            children: BTreeMap::new(),
            parent,
            name: name.to_string(),
        };
        let ino = match self.inodes.iter().position(|i| i.is_none()) {
            Some(free) => {
                self.inodes[free] = Some(node);
                free
            }
            None => {
                self.inodes.push(Some(node));
                self.inodes.len() - 1
            }
        };
        self.inode_mut(parent)
            .unwrap()
            .children
            .insert(name.to_string(), ino);
        Ok(ino)
    }

    pub fn mkdir_p(&mut self, path: &str) {
        let mut cur = String::new();
        for part in path.split('/').filter(|p| !p.is_empty()) {
            cur.push('/');
            cur.push_str(part);
            let _ = self.create(&cur, Kind::Dir);
        }
    }

    pub fn write_file(&mut self, path: &str, data: &[u8]) -> Result<Ino, FsError> {
        let ino = match self.lookup(path) {
            Ok(i) => i,
            Err(FsError::NotFound) => self.create(path, Kind::File)?,
            Err(e) => return Err(e),
        };
        let node = self.inode_mut(ino).unwrap();
        if node.kind != Kind::File {
            return Err(FsError::IsDir);
        }
        node.data.clear();
        node.data.extend_from_slice(data);
        Ok(ino)
    }

    pub fn remove(&mut self, ino: Ino) -> Result<(), FsError> {
        if ino == ROOT {
            return Err(FsError::BadPath);
        }
        let node = self.inode(ino).ok_or(FsError::NotFound)?;
        if !node.children.is_empty() {
            return Err(FsError::NotEmpty);
        }
        let (parent, name) = (node.parent, node.name.clone());
        if let Some(p) = self.inode_mut(parent) {
            p.children.remove(&name);
        }
        self.inodes[ino] = None;
        Ok(())
    }

    pub fn read_at(&self, ino: Ino, off: usize, buf: &mut [u8]) -> Result<usize, FsError> {
        let node = self.inode(ino).ok_or(FsError::NotFound)?;
        if node.kind != Kind::File {
            return Err(FsError::IsDir);
        }
        if off >= node.data.len() {
            return Ok(0);
        }
        let n = buf.len().min(node.data.len() - off);
        buf[..n].copy_from_slice(&node.data[off..off + n]);
        Ok(n)
    }

    /// Write and hand back what was there before, so the caller can undo it.
    pub fn write_at(
        &mut self,
        ino: Ino,
        off: usize,
        src: &[u8],
    ) -> Result<super::undo::UndoRecord, FsError> {
        let node = self.inode_mut(ino).ok_or(FsError::NotFound)?;
        if node.kind != Kind::File {
            return Err(FsError::IsDir);
        }
        let old_size = node.data.len();
        let overlap_end = (off + src.len()).min(old_size);
        let old_bytes = if off < overlap_end {
            node.data[off..overlap_end].to_vec()
        } else {
            Vec::new()
        };
        if node.data.len() < off + src.len() {
            node.data.resize(off + src.len(), 0);
        }
        node.data[off..off + src.len()].copy_from_slice(src);
        Ok(super::undo::UndoRecord::Write {
            ino,
            off,
            old_bytes,
            old_size,
        })
    }

    pub fn truncate(&mut self, ino: Ino, len: usize) -> Result<super::undo::UndoRecord, FsError> {
        let node = self.inode_mut(ino).ok_or(FsError::NotFound)?;
        let old_bytes = if len < node.data.len() {
            node.data[len..].to_vec()
        } else {
            Vec::new()
        };
        let old_size = node.data.len();
        node.data.resize(len, 0);
        Ok(super::undo::UndoRecord::Write {
            ino,
            off: len,
            old_bytes,
            old_size,
        })
    }

    pub fn size(&self, ino: Ino) -> usize {
        self.inode(ino).map(|n| n.data.len()).unwrap_or(0)
    }

    /// Restore a file to an older state (used by the undo log).
    pub fn rollback_write(&mut self, ino: Ino, off: usize, old: &[u8], old_size: usize) {
        if let Some(node) = self.inode_mut(ino) {
            if node.data.len() < off + old.len() {
                node.data.resize(off + old.len(), 0);
            }
            node.data[off..off + old.len()].copy_from_slice(old);
            node.data.resize(old_size, 0);
        }
    }

    pub fn list(&self, ino: Ino) -> Result<Vec<(String, Kind, usize)>, FsError> {
        let node = self.inode(ino).ok_or(FsError::NotFound)?;
        if node.kind != Kind::Dir {
            return Err(FsError::NotDir);
        }
        Ok(node
            .children
            .iter()
            .map(|(n, &i)| {
                let c = self.inode(i).unwrap();
                (n.clone(), c.kind, c.data.len())
            })
            .collect())
    }

    pub fn total_bytes(&self) -> usize {
        self.inodes.iter().flatten().map(|n| n.data.len()).sum()
    }
}
