mod files;

extern crate clap;
use chrono::{NaiveDate, NaiveDateTime};
use clap::Parser;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{PathBuf};

struct CopyStats {
	bytes: u64,
	files: u64,
}

// TODO should be able to exclude symlinks/hardlinks/junctions from the source directories
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
	#[clap(short = 's', long = "stats", help = "If present, will append stats to this file as comma-separated values (date,total_bytes_copied,total_files_copied)")]
	path_stats: Option<String>,
	#[clap(short = 'm', long = "min-diff", default_value_t = 1, help = "If the file modification time differs by at least this many seconds, the file will be backed up")]
	min_diff_secs: u64,
	#[clap(short = 'v', long = "verbose", help = "If present, will output information for all links created")]
	verbose: bool,
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
	let mut prev_files_collector = files::CollectorFileHandler{files: HashMap::new()};

	if let Some(prev_base_dir) = maybe_prev_base_dir {
		match files::handle_files_recursive(&prev_base_dir, &excluded_names, &mut prev_files_collector) {
			Ok(_) => {
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
		println!("First backup, everything will be copied");
	}
	let prev_files = prev_files_collector.files;

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

		match std::fs::create_dir_all(&dest_dir){
			Ok(_) =>{}
			Err(err) => {
				println!("Failed to create directory {} because {}", &dest_dir.display(), err);
				return;
			}
		}

		println!(
			"Backing up \"{}\" to \"{}\"",
			&src_base_dir.display(),
			&dest_dir.display()
		);
		
		let result = 
		match maybe_prev_dir {
			Some(prev_dir) => {
				let mut handler = files::LinkOrCopyFileHandler
				{
					prev_files: &prev_files,
					src_base_dir: &src_base_dir,
					dest_dir: &dest_dir,
					prev_dir: &prev_dir,
					min_diff_secs: args.min_diff_secs,
					bytes_copied: 0,
					files_copied: 0,
					verbose: args.verbose,
				};
				match files::handle_files_recursive(&src_base_dir, &excluded_names, &mut handler) {
					Ok(_) => {Ok(CopyStats{bytes: handler.bytes_copied, files: handler.files_copied})}
					Err(err) => {Err(err)}
				}
			}
			None => {
				let mut handler = files::CopyFileHandler
				{
					src_base_dir: &src_base_dir,
					dest_dir: &dest_dir,
					bytes_copied: 0,
					files_copied: 0,
				};
				match files::handle_files_recursive(&src_base_dir, &excluded_names, &mut handler) {
					Ok(_) => {Ok(CopyStats{bytes: handler.bytes_copied, files: handler.files_copied})}
					Err(err) => {Err(err)}
				}
			}
		};
		match result {
			Ok(stats) => {
				total_bytes_copied += stats.bytes;
				total_files_copied += stats.files;
			}
			Err(err) => {
				println!("Error occurred for source directory {} while doing backup: {}", src_base_dir.display(), err);
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