//! The lib for markdown based static HTML wiki generation

#[macro_use]
extern crate log;
extern crate glob;
extern crate iron;
extern crate markdown;
extern crate mowl;
#[macro_use]
extern crate error_chain;
extern crate sha_1;

pub mod error;

use error::*;
use glob::glob;
use log::LogLevel;
use markdown::to_html;

use iron::prelude::*;
use iron::status;
use iron::headers::ContentType;

use std::fs::{self, canonicalize, create_dir_all, File};
use std::path::{Path, PathBuf, MAIN_SEPARATOR};
use std::io::BufReader;
use std::io::prelude::*;
use std::str;
use sha_1::{Sha1, Digest};

static SHA_FILE: &str = ".files.sha";

struct InputPaths {
    path: PathBuf,
    hash: String,
}

impl InputPaths {
    fn new(path: &str) -> Self {
        InputPaths {
            path: PathBuf::from(path),
            hash: String::new(),
        }
    }
}

#[derive(Default)]
/// Global processing structure
pub struct Wiki {
    /// A collection of input_paths for the processing
    input_paths: Vec<InputPaths>,
    /// The html output paths
    output_paths: Vec<PathBuf>,
}

impl Wiki {
    /// Create a new `Wiki` instance
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new instance of the processing lib
    pub fn init_logging(&mut self, level: LogLevel) -> Result<()> {
        // Init logger crate
        match mowl::init_with_level(level) {
            Ok(_) => info!("Log level set to: {}", level),
            Err(_) => bail!("Initialization of mowl logger failed."),
        }

        Ok(())
    }

    /// Reads all markdown files recursively from a given directory.
    /// Clears the current available input_paths
    pub fn read_from_directory(&mut self, directory: &str) -> Result<()> {
        /// Remove all input_paths
        self.input_paths.clear();

        /// Gather new content
        let md_path = PathBuf::from(&directory).join("**").join("*.md");
        if !Path::new(&directory).is_dir() {
            bail!("The path '{}' does not exist", directory);
        }

        /// Use the current working directory as a fallback
        for entry in glob(md_path.to_str().unwrap_or("."))? {
            self.input_paths.push(
                InputPaths::new(entry?.to_str()
                                    .ok_or_else(|| "Unable to stringfy entry in markdown path.")?));
        }

        Ok(())
    }

    /// Print absolute path of all added md files
    pub fn list_current_input_paths(&self) {
        info!("Found the following markdown files:");
        for file in &self.input_paths {
            println!("    - {:?}", file.path);
        }
    }


    /// Reads the file hash for the file specified by `file_str` out of `hash_file_str`
    fn read_file_hash(hash_file_str: &str, file_str: &str) -> Option<(String)> {
        let hash_file_res = File::open(hash_file_str);
        if hash_file_res.is_err() {
            return None;
        }
        let hash_file_reader = BufReader::new(hash_file_res.unwrap());
        for line in hash_file_reader.lines() {
            match line {
                Ok(l) => {

                    // Break the line between `<hash>:<file>`
                    let sha_args: Vec<&str> = l.split(':').collect();

                    // File matched
                    if sha_args[1] == file_str {
                        return Some(String::from(sha_args[0]));
                    }
                },
                Err(_) => {},
            }
        }
        None
    }

    /// Writes all input files and their hashes into the file `hash_file_str`
    fn write_file_hash(&mut self, hash_file_str: &str) -> Result<()> {
        // Renew the hash_file
        if Path::new(hash_file_str).exists() {
            fs::remove_file(hash_file_str)?;
        }
        let mut hash_file = File::create(hash_file_str)?;

        // Write content into file in form `<hash>:<file>`
        let mut hash_file_content = String::new();
        for input_path in &self.input_paths {
            hash_file_content.push_str(format!("{}:{}\n",
                                               input_path.hash.as_str(),
                                               input_path.path.to_str()
                                               .ok_or_else(|| "Unable to stringfy input path.")?)
                                       .as_str());
        }
        hash_file.write_all(hash_file_content.as_bytes())?;

        Ok (())
    }

    /// Calculate the hash of the given `file_str`
    fn get_file_hash(file_str: &str) -> Result<(String)> {
        let mut sha1 = Sha1::default();
        let mut buffer = String::new();
        let mut file_instance = File::open(PathBuf::from(file_str))?;

        file_instance.read_to_string(&mut buffer)?;

        sha1.input(buffer.as_bytes());
        let file_hash = sha1.result();

        let mut hash_str = String::new();
        for hash_byte in file_hash {
            hash_str.push_str(format!("{:x}", hash_byte).as_str());
        }
        debug!("Calculated file hash: {}", hash_str);

        Ok(hash_str)
    }

    /// Checks whether the calculated hash of `file_str` is equal to the hash stored
    /// in the file `hash_file_str`
    fn check_hash_currency(hash_file_str: &str, file_str: &str) -> Result<String> {
        debug!("Check hash currency of '{}'", file_str);
        let current_file_hash = Wiki::get_file_hash(file_str)?;
        match Wiki::read_file_hash(hash_file_str, file_str) {
            Some(stored_file_hash) => {
                // Stored file hash was found
                debug!("Extracted file hash:  {}", stored_file_hash);

                // Calculated hash of current file equals stored hash?
                if current_file_hash != stored_file_hash {
                    return Err(Error::from(current_file_hash));
                } else {
                    return Ok(current_file_hash);
                }
            },
            None => {
                // No stored hash found for this file
                return Err(Error::from(current_file_hash));
            },
        }
    }

    /// Read the content of all files and convert it to HTML
    pub fn read_content_from_current_paths(&mut self, input_root_dir: &str,
                                           output_directory: &str) -> Result<()> {
        // Check whether output_directory exists, if not -> create
        if !Path::new(output_directory).exists() {
            info!("Creating directory for HMTL output: '{}'.", output_directory);
            fs::create_dir(output_directory)?;
        }

        let sha_file_path = PathBuf::from(output_directory).join(SHA_FILE);
        let sha_file = sha_file_path.to_str()
                           .ok_or_else(|| "Unable to stringify the sha file path.")?;

        // Iterate over all available input_paths
        for file in &mut self.input_paths {
            info!("Parsing file: {}", file.path.display());

            // Open the file and read its content
            let mut f = File::open(&file.path)?;
            let mut buffer = String::new();
            f.read_to_string(&mut buffer)?;

            // Creating the related HTML file in output_directory
            match file.path.to_str() {
                Some(file_str) => {
                    // Get canonical normal forms of the input path and the recursively
                    // searched directories
                    let file_buf_n = canonicalize(&PathBuf::from(file_str))?;
                    let file_str_n = file_buf_n.to_str()
                                     .ok_or_else(|| "Unable to stringify canonical normal form of md-file.")?;
                    let input_root_buf_n = canonicalize(&PathBuf::from(input_root_dir))?;
                    let mut input_root_str_n = String::from(
                        input_root_buf_n.to_str()
                        .ok_or_else(|| "Unable to stringify canonical normal form of input root.")?
                    );

                    // Add native seperator to avoid getting the wrong path
                    input_root_str_n.push(MAIN_SEPARATOR);

                    // Reduce the input dir and replace the extension
                    let output_str = String::from(file_str_n)
                        .replace(input_root_str_n.as_str(), "")
                        .replace(".md", ".html");
                    let output_path = Path::new(output_str.as_str());

                    match output_path.parent() {
                        Some(parent) => {
                            // Creating folder structure if neccessary
                            let parent_path = Path::new(output_directory)
                                .join(parent.to_str().unwrap_or("."));
                            create_dir_all(parent_path)?;
                        },
                        None => bail!("Can't get output path parent."),
                    }

                    match Wiki::check_hash_currency(sha_file, file_str) {
                        Ok(hash) => {
                            // File hash is up to date, no need to rebuild
                            file.hash = hash;
                            debug!("File '{}' hash up to date.", file_str);
                        },
                        Err(hash) => {
                            // Creating the ouput HTML file
                            file.hash = hash.to_string();
                            let output_file_path = PathBuf::from(&output_directory)
                                                       .join(output_path);
                            let mut output_file = File::create(output_file_path.to_owned())?;
                            output_file.write(to_html(&buffer).as_bytes())?;
                        },
                    }
                    self.output_paths.push(output_path.to_path_buf());
                },
                None => bail!("Can not stringfy file path"),
            }
        }

        self.write_file_hash(sha_file)?;

        Ok(())
    }

    /// Creates an index.html with simple tree structure view when no index.md was seen
    pub fn create_index_tree(&self, output_directory: &str) -> Result<()> {
        let index_path = Path::new(output_directory).join("index.html");
        if !index_path.exists() {
            info!("Creating index.html at {}",
                  index_path.to_str().ok_or_else(|| "Unable to stringify index path.")?);
            let mut index_file = File::create(index_path)?;
            let mut index_str = String::from(include_str!("html/index.template.html"));
            for output_path in &self.output_paths {
                index_str.push_str(format!("<li><a href=\"{}\">{}</a></li>\n",
                                           output_path.to_str()
                                               .ok_or_else(|| "Unable to stringify output path.")?,
                                           output_path.file_name()
                                               .ok_or_else(|| "Unable to extract file name for path")?
                                               .to_str().ok_or_else(|| "Unable to stringify output path.")?)
                                   .as_str());
            }
            index_file.write_all(index_str.as_bytes())?;
        }

        Ok(())
    }

    /// Create an HTTP server serving the generated files
    pub fn serve(&self, output_directory: &str) -> Result<()> {
        // Create a default listening address
        let addr = "localhost:5000";
        info!("Listening on {}", addr);

        // Moving the data into the closure
        let output_directory_string = output_directory.to_owned();

        // Create a new iron instance
        Iron::new(move |request: &mut Request| {
                // The owned path needs to created from the cloned string
                let mut path = PathBuf::from(output_directory_string.clone());

                // Create the full path
                for part in request.url.path() {
                    path.push(part);
                }

                // Could use some security validation for the path here.

                // Use a default page for the middleware
                if path.is_dir() {
                    path.push("index.html");
                }

                let mut f = match File::open(path) {
                    Ok(v) => v,
                    _ => return Ok(Response::with((ContentType::html().0,
                                                   status::NotFound,
                                                   include_str!("html/404.html")))),
                };

                let mut buffer = String::new();
                match f.read_to_string(&mut buffer) {
                    Ok(v) => v,
                    _ => return Ok(Response::with((ContentType::html().0,
                                                   status::InternalServerError,
                                                   include_str!("html/500.html")))),
                };

                // Content type needs to be determined from the file rather
                // than assuming html
                Ok(Response::with((ContentType::html().0, status::Ok, buffer)))

            }).http(addr)?;

        Ok(())
    }
}
