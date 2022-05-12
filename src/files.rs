use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{DirEntry};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct FileMeta {
   pub len: u64,
   pub modified: SystemTime,
}
pub struct FilesAndDirs {
   pub files: HashMap<PathBuf, FileMeta>,
   pub dirs: Vec<PathBuf>,
}

pub fn get_files_recursive(
   base_path: &Path,
   excluded: &HashSet<OsString>,
) -> std::io::Result<FilesAndDirs> {
   let mut files: HashMap<PathBuf, FileMeta> = HashMap::new();
   let mut dirs: Vec<PathBuf> = Vec::new();
   let mut all_dirs: Vec<PathBuf> = Vec::new();
   dirs.push(PathBuf::from(base_path));

   while let Some(path) = dirs.pop() {
      for entry in std::fs::read_dir(path)? {
         let entry: DirEntry = entry?;
         let meta = entry.metadata()?;
         if !excluded.contains(&entry.file_name()) {
            if meta.is_dir() {
               all_dirs.push(entry.path());
               dirs.push(entry.path());
            } else if meta.is_file() {
               files.insert(
                  entry.path(),
                  FileMeta {
                     len: meta.len(),
                     modified: meta.modified()?,
                  },
               );
            }
         }
      }
   }

   Ok(FilesAndDirs {
      files: files,
      dirs: all_dirs,
   })
}

