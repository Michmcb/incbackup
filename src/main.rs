mod files;
extern crate clap;
use chrono::{NaiveDate, NaiveDateTime};
use clap::Parser;
use files::FileMeta;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, SystemTimeError};

struct CopyStats {
	bytes: u64,
	files: u64,
}

// TODO Don't get all the files at once, instead make an iterator that returns a vec of all files in a directory, and it does this recursively.
// That way, we only get one directory's worth of files at a time for less memory pressure and also we can notice changes that happen midway
// through the backup if they're snuck in fast enough. It also makes the backup run in a more predictable order.
// TODO should be able to exclude symlinks/hardlinks/junctions from the source directories
// TODO toggle making hardlinks or not when the file is unchanged (default: true, so the switch should be like -nl)
#[derive(Parser, Debug)]
#[clap(name = "incbackup")]
#[clap(author = "Michael McBride")]
#[clap(version, about, long_about = None)]
struct Arguments {
	backup_path: String,
	#[clap(short = 'd', long = "dir", help = "The source directories to be included in the backup")]
	src_dirs: Vec<String>,
	#[clap(short = 'x', long = "exclude", help = "Any files or directories with one of these names will be excluded from the backup")]
	excluded_names: Option<Vec<String>>,
	#[clap(short = 's', long = "stats", help = "If specified, will append stats to this file as comma-separated values (date,total_bytes_copied,total_files_copied)")]
	path_stats: Option<String>,
	#[clap(short = 'm', long = "min-diff", default_value_t = 1, help = "If the file modification time differs by at least this many seconds, the file will be backed up")]
	min_diff_secs: u64,
}

fn main() {
	let args = Arguments::parse();
	let backup_path = args.backup_path;
	let src_dirs = args.src_dirs;
	let mut excluded_names = HashSet::new();
	if let Some(en) = args.excluded_names {
		for elem in en.iter() {
			excluded_names.insert(OsString::from(elem));
		}
	}

	let backup_path_buf = PathBuf::from(&backup_path);
	if !backup_path_buf.exists() {
		match std::fs::create_dir_all(&backup_path_buf) {
			Ok(_) => {
				println!("Directory {} created", &backup_path);
			}
			Err(err) => {
				println!("Error creating directory ({}): {}", &backup_path, &err);
				return;
			}
		}
	}
	drop(backup_path_buf);

	let date_dirs;
	match get_dirs(&backup_path) {
		Ok(ok) => {
			date_dirs = ok;
		}
		Err(err) => {
			println!("Error reading directory ({}): {}", &backup_path, &err);
			return;
		}
	}

	// Now we want to get the latest backup date/time
	let latest_date = date_dirs.iter().fold(
		NaiveDate::from_ymd(1, 1, 1).and_hms(0, 0, 0),
		|latest, (&key, _)| {
			if key > latest {
				key
			} else {
				latest
			}
		},
	);

	// It might not actually exist, if there's no folders in the directory yet (i.e. first backup)
	// In thay case we just need to copy every single file.
	let maybe_prev_base_dir = date_dirs.get(&latest_date);

	// Now create another directory for the backup done at this time
	let mut backup_base_dir_working = PathBuf::from(&backup_path);
	let orig_name = chrono::Local::now().format("%Y-%m-%d %H-%M-%S").to_string();
	let mut progress_name = String::from(&orig_name);
	progress_name.push_str("-inprogress");
	backup_base_dir_working.push(progress_name);

	let mut backup_base_dir = PathBuf::from(&backup_path);
	backup_base_dir.push(&orig_name);

	if !backup_base_dir_working.exists() {
		match std::fs::create_dir(&backup_base_dir_working) {
			Ok(_) => {
				println!("Backup directory: {}", &backup_base_dir_working.display());
			}
			Err(err) => {
				println!(
					"Failed to create new directory for today's backup: {}",
					&err
				);
				return;
			}
		}
	}
	let dest_base_dir = &backup_base_dir_working;
	let prev_files;

	if let Some(prev_base_dir) = maybe_prev_base_dir {
		match files::get_files_recursive(&prev_base_dir, &excluded_names) {
			Ok(ok) => {
				prev_files = ok.files;
				println!("Previous backup directory: {}", &prev_base_dir.display());
			}
			Err(err) => {
				println!(
					"Error reading backup directory {}: {}",
					&prev_base_dir.display(),
					&err
				);
				return;
			}
		}
	} else {
		prev_files = HashMap::new();
		println!("First backup, everything will be copied");
	}

	let mut total_bytes_copied = 0u64;
	let mut total_files_copied = 0u64;
	for src_base_dir in src_dirs {
		let src_base_dir = PathBuf::from(src_base_dir);

		let mut dest_dir = PathBuf::new();
		match dest_dir_from_src_leaf_dir(&src_base_dir, dest_base_dir, &mut dest_dir) {
			Some(_) => {}
			None => {
				dest_dir = PathBuf::from(dest_base_dir);
			}
		}
		let maybe_prev_dir;
		if let Some(prev_base_dir) = maybe_prev_base_dir {
			let mut prev_dir = PathBuf::new();
			match dest_dir_from_src_leaf_dir(&src_base_dir, prev_base_dir, &mut prev_dir) {
				Some(_) => {}
				None => {
					prev_dir = PathBuf::from(prev_base_dir);
				}
			}
			maybe_prev_dir = Some(prev_dir);
		} else {
			maybe_prev_dir = None;
		}

		println!(
			"Backing up \"{}\" to \"{}\"",
			&src_base_dir.display(),
			&dest_dir.display()
		);

		match files::get_files_recursive(&src_base_dir, &excluded_names) {
			Ok(src_files_dirs) => {
				// First, we need to create all the directories
				println!("Creating {} directories...", src_files_dirs.dirs.len());
				for src_dir_path in src_files_dirs.dirs.iter() {
					let src_dir = src_dir_path.strip_prefix(&src_base_dir).unwrap(); // TODO don't panic here
					let mut dir_to_create = PathBuf::from(&dest_dir);
					dir_to_create.push(&src_dir);
					if !dir_to_create.exists() {
						match std::fs::create_dir_all(&dir_to_create) {
							Ok(_) => {}
							Err(err) => {
								println!(
									"Failed to create directory ({}): {}",
									&dir_to_create.display(),
									&err
								);
								return;
							}
						}
					}
				}
				println!("Done!");

				println!(
					"There are {} files to check...",
					&src_files_dirs.files.len()
				);
				let stats = match maybe_prev_dir {
					Some(prev_dir) => copy_or_hardlink_files(
						&src_files_dirs.files,
						&prev_files,
						&src_base_dir,
						&dest_dir,
						&prev_dir,
						args.min_diff_secs,
					),
					None => copy_files(&src_files_dirs.files, &src_base_dir, &dest_dir),
				};
				total_bytes_copied += stats.bytes;
				total_files_copied += stats.files;
			}
			Err(err) => {
				println!("\x1b[mError reading directory: {}", &err);
				return;
			}
		}
	}

	match std::fs::rename(&backup_base_dir_working, &backup_base_dir) {
		Ok(_) => {}
		Err(err) => {
			println!(
				"Failed to remove -inprogress from directory {} because {}",
				backup_base_dir_working.display(),
				err
			);
		}
	}

	println!("\x1b[mTotal bytes copied: {}", &total_bytes_copied);
	println!("Total files copied: {}", &total_files_copied);
	match args.path_stats {
		Some(path) => {
			let path = PathBuf::from(path);
			if let Some(parent) = path.parent() {
				match std::fs::create_dir_all(&parent) {
					Ok(_) => {}
					Err(err) => {
						println!(
							"Failed to create directory for stats file {} because {}",
							&parent.display(),
							err
						);
					}
				}
			}
			match OpenOptions::new()
				.create(true)
				.write(true)
				.append(true)
				.open(&path)
			{
				Ok(mut file) => {
					match write!(
						file,
						"{},{},{}\n",
						&orig_name, &total_bytes_copied, &total_files_copied
					) {
						Ok(_) => {}
						Err(err) => {
							println!(
								"Failed to write to stats file {} because {}",
								&path.display(),
								err
							);
						}
					}
				}
				Err(err) => {
					println!(
						"Failed to open/create stats file {} because {}",
						&path.display(),
						err
					);
				}
			}
		}
		None => {}
	}
}

fn get_dirs(path: &str) -> std::io::Result<HashMap<NaiveDateTime, PathBuf>> {
	let mut dates: HashMap<NaiveDateTime, PathBuf> = HashMap::new();
	for entry in std::fs::read_dir(path)? {
		let entry = entry?;

		if let Some(name) = entry.file_name().to_str() {
			match chrono::NaiveDateTime::parse_from_str(name, "%Y-%m-%d %H-%M-%S") {
				Ok(date) => {
					dates.insert(date, entry.path());
				}
				Err(_) => {}
			}
		}
	}

	Ok(dates)
}

fn dest_dir_from_src_leaf_dir(src: &PathBuf, dest: &PathBuf, buf: &mut PathBuf) -> Option<()> {
	buf.push(dest);
	if let Some(src_parent) = src.parent() {
		// Failing to strip a prefix that is a parent should never fail, but you never know...
		let sub_dir = src.strip_prefix(src_parent).unwrap(); // TODO don't panic here
		buf.push(sub_dir);
		return Some(());
	}
	return None;
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

fn copy_files(
	files: &HashMap<PathBuf, FileMeta>,
	src_base_dir: &Path,
	dest_dir: &Path,
) -> CopyStats {
	let mut bytes: u64 = 0;
	let mut num: u64 = 0;
	for (src_path, _) in files.iter() {
		let src_file = src_path.strip_prefix(&src_base_dir).unwrap(); // TODO don't panic here
		let mut dest_path = PathBuf::from(&dest_dir);
		dest_path.push(&src_file);

		match std::fs::copy(&src_path, &dest_path) {
			Ok(bytes_copied) => {
				bytes += bytes_copied;
				num += 1;
				println!("\x1b[92mCopied: {}", &src_path.display());
			}
			Err(err) => {
				println!("\x1b[91mFailed to copy file: {}", &err);
			}
		}
	}

	CopyStats {
		bytes: bytes,
		files: num,
	}
}

fn copy_or_hardlink_files(
	files: &HashMap<PathBuf, FileMeta>,
	prev_files: &HashMap<PathBuf, FileMeta>,
	src_base_dir: &Path,
	dest_dir: &Path,
	prev_dir: &Path,
	min_diff_secs: u64
) -> CopyStats {
	let mut bytes: u64 = 0;
	let mut num: u64 = 0;
	for (src_path, src_meta) in files.iter() {
		// Now, for each of the directories we have in the source, we need to check recursively all of the files etc.
		// And there's outcomes...
		// Same length/modified, make hardlink
		// Different length/modified, copy new
		// New file, copy
		// Because the backup folder is always empty to begin with, we don't have to worry about deleting anything

		let src_file = src_path.strip_prefix(&src_base_dir).unwrap(); // TODO don't panic here
		let mut dest_path = PathBuf::from(&dest_dir);
		dest_path.push(&src_file);
		let mut prev_path = PathBuf::from(&prev_dir);
		prev_path.push(&src_file);

		let copy;

		if let Some(prev_meta) = prev_files.get(&prev_path) {
			if src_meta.len == prev_meta.len {
				// lengths are the same so compare the modification times
				match diff_secs(&src_meta.modified, &prev_meta.modified) {
					Ok(secs) => {
						// If the modification time in seconds differs by at least 2 seconds, then assume it has changed and needs to be copied
						copy = secs >= min_diff_secs;
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
					bytes += bytes_copied;
					num += 1;
					println!("\x1b[92mChanged: {}", &src_path.display());
				}
				Err(err) => {
					println!("\x1b[91mFailed to copy file: {}", &err);
				}
			}
		} else {
			match std::fs::hard_link(&prev_path, &dest_path) {
				Ok(_) => {}
				Err(err) => {
					println!("\x1b[91mFailed to hardlink file: {}", err);
				}
			}
		}
	}
	// Reset the colours
	print!("\x1b[m");
	CopyStats {
		bytes: bytes,
		files: num,
	}
}
