use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{DirEntry};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, SystemTimeError};

pub struct FileMeta {
   pub len: u64,
   pub modified: SystemTime,
}

pub trait FileHandler {
   fn file(&mut self, file: &std::fs::DirEntry, meta: &std::fs::Metadata) -> std::io::Result<()>;
   fn dir(&mut self, dir: &std::fs::DirEntry, meta: &std::fs::Metadata) -> std::io::Result<()>;
}

pub struct CollectorFileHandler {
   pub files: HashMap<PathBuf, FileMeta>,
}

impl FileHandler for CollectorFileHandler{
   fn file(&mut self, file: &std::fs::DirEntry, meta: &std::fs::Metadata) -> std::io::Result<()> {
      self.files.insert(file.path(), FileMeta { len: meta.len(), modified: meta.modified().unwrap(), });
      Ok(())
   }
   fn dir(&mut self, _dir: &std::fs::DirEntry, _meta: &std::fs::Metadata) -> std::io::Result<()> {
      Ok(())
   }
}

pub struct LinkOrCopyFileHandler<'a>{
	pub prev_files: &'a HashMap<PathBuf, FileMeta>,
	pub src_base_dir: &'a Path,
	pub dest_dir: &'a Path,
	pub prev_dir: &'a Path,
	pub min_diff_secs: u64,
   pub bytes_copied: u64,
   pub files_copied: u64,
	pub verbose: bool,
}

impl<'a> FileHandler for LinkOrCopyFileHandler<'a>{
   fn file(&mut self, file: &std::fs::DirEntry, meta: &std::fs::Metadata) -> std::io::Result<()> {
      // Now, for each of the directories we have in the source, we need to check recursively all of the files etc.
		// And there's outcomes...
		// Same length/modified, make hardlink
		// Different length/modified, copy new
		// New file, copy
		// Because the backup folder is always empty to begin with, we don't have to worry about deleting anything
      let src_path = file.path();
		let src_file = src_path.strip_prefix(self.src_base_dir).unwrap(); // TODO don't panic here
		let mut dest_path = PathBuf::from(self.dest_dir);
		dest_path.push(&src_file);
		let mut prev_path = PathBuf::from(self.prev_dir);
		prev_path.push(&src_file);
      let src_meta = meta;

		let copy;

		if let Some(prev_meta) = self.prev_files.get(&prev_path) {
			if src_meta.len() == prev_meta.len {
				// lengths are the same so compare the modification times
				match diff_secs(&src_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), &prev_meta.modified) {
					Ok(secs) => {
						// If the modification time in seconds differs by at least 2 seconds, then assume it has changed and needs to be copied
						copy = secs >= self.min_diff_secs;
					}
					Err(err) => {
						// Can't tell, so be conservative and assume it's changed
						println!(
							"\x1b[mCannot compare filetimes ({}), assuming file has changed",
							&err
						);
						copy = true;
					}
				}
			} else {
				// lengths are different, file's definitely changed
				copy = true;
			}
		} else {
			// File is new, copy the file to the new directory
			copy = true;
		}

		if copy {
			match std::fs::copy(&src_path, &dest_path) {
				Ok(bytes_copied) => {
					self.bytes_copied += bytes_copied;
					self.files_copied += 1;
					println!("\x1b[92mChanged:\x1b[m {}", &src_path.display());
               return Ok(());
				}
				Err(err) => {
					println!("\x1b[91mFailed to copy file:\x1b[m {}", &err);
               return Err(err);
				}
			}
		} else {
			match std::fs::hard_link(&prev_path, &dest_path) {
				Ok(_) => {
					if self.verbose {
						println!("\x1b[93mLinked:\x1b[m {}", &src_path.display());
					}
               return Ok(());
            }
				Err(err) => {
					println!("\x1b[91mFailed to hardlink file:\x1b[m {}", err);
               return Err(err);
				}
			}
		}
   }
   fn dir(&mut self, dir: &std::fs::DirEntry, _: &std::fs::Metadata) -> std::io::Result<()> {
      make_dir(&dir.path(), self.src_base_dir, self.dest_dir)
   }
}

pub struct CopyFileHandler<'a>{
   pub src_base_dir: &'a Path,
   pub dest_dir: &'a Path,
   pub bytes_copied: u64,
   pub files_copied: u64,
}

impl<'a> FileHandler for CopyFileHandler<'a>{
   fn file(&mut self, file: &std::fs::DirEntry, _meta: &std::fs::Metadata) -> std::io::Result<()> {
      let src_path = file.path();
      let src_file = src_path.strip_prefix(self.src_base_dir).unwrap(); // TODO don't panic here
		let mut dest_path = PathBuf::from(self.dest_dir);
		dest_path.push(&src_file);
      match std::fs::copy(&src_path, &dest_path) {
			Ok(bytes_copied) => {
				self.bytes_copied += bytes_copied;
				self.files_copied += 1;
				println!("\x1b[92mCopied:\x1b[m {}", &src_path.display());
            return Ok(());
			}
			Err(err) => {
				println!("\x1b[91mFailed to copy file:\x1b[m {}", &err);
            return Err(err);
			}
		}
   }
   fn dir(&mut self, dir: &std::fs::DirEntry, _meta: &std::fs::Metadata) -> std::io::Result<()> {
      make_dir(&dir.path(), self.src_base_dir, self.dest_dir)
   }
}

fn make_dir(src_dir: &Path, src_base_dir: &Path, dest_dir: &Path) -> std::io::Result<()> {
   let src_dir = src_dir.strip_prefix(src_base_dir).unwrap(); // TODO don't panic here
   let mut dir_to_create = PathBuf::from(dest_dir);
   dir_to_create.push(&src_dir);
   if !dir_to_create.exists() {
      std::fs::create_dir_all(&dir_to_create)?;
   }
   Ok(())
}

fn diff_secs(t1: &SystemTime, t2: &SystemTime) -> Result<u64, SystemTimeError> {
	let seconds1 = t1.duration_since(SystemTime::UNIX_EPOCH)?.as_secs();
	let seconds2 = t2.duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

	// Return the absolute difference in seconds
	if seconds1 > seconds2 {
		Ok(seconds1 - seconds2)
	} else {
		Ok(seconds2 - seconds1)
	}
}

pub fn handle_files_recursive(
   base_path: &Path,
   excluded: &HashSet<OsString>,
   handler: &mut dyn FileHandler,
) -> std::io::Result<()> {
   let mut dirs: Vec<PathBuf> = Vec::new();
   dirs.push(PathBuf::from(base_path));

   while let Some(path) = dirs.pop() {
      for entry in std::fs::read_dir(path)? {
         let entry: DirEntry = entry?;
         let meta = entry.metadata()?;
         if !excluded.contains(&entry.file_name()) {
            if meta.is_dir() {
               handler.dir(&entry, &meta)?;
               dirs.push(entry.path());
            } else if meta.is_file() {
               handler.file(&entry, &meta)?;
            }
         }
      }
   }
   Ok(())
}