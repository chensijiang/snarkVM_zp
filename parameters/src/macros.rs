// Copyright (C) 2019-2022 Aleo Systems Inc.
// This file is part of the snarkVM library.

// The snarkVM library is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// The snarkVM library is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with the snarkVM library. If not, see <https://www.gnu.org/licenses/>.

#[macro_export]
macro_rules! checksum {
    ($bytes: expr) => {{
        use sha2::Digest;
        hex::encode(&sha2::Sha256::digest($bytes))
    }};
}

#[macro_export]
macro_rules! checksum_error {
    ($expected: expr, $candidate: expr) => {
        Err($crate::errors::ParameterError::ChecksumMismatch($expected, $candidate))
    };
}

macro_rules! impl_store_and_remote_fetch {
    () => {
        #[cfg(not(feature = "wasm"))]
        fn store_bytes(buffer: &[u8], file_path: &std::path::Path) -> Result<(), $crate::errors::ParameterError> {
            use snarkvm_utilities::Write;

            #[cfg(not(feature = "no_std_out"))]
            {
                use colored::*;
                let output = format!("{:>15} - Storing file in {:?}", "Installation", file_path);
                println!("{}", output.dimmed());
            }

            // Ensure the folders up to the file path all exist.
            let mut directory_path = file_path.to_path_buf();
            directory_path.pop();
            let _ = std::fs::create_dir_all(directory_path)?;

            // Attempt to write the parameter buffer to a file.
            match std::fs::File::create(file_path) {
                Ok(mut file) => file.write_all(&buffer)?,
                Err(error) => eprintln!("{}", error),
            }
            Ok(())
        }

        #[cfg(not(feature = "wasm"))]
        fn remote_fetch(buffer: &mut Vec<u8>, url: &str) -> Result<(), $crate::errors::ParameterError> {
            let mut easy = curl::easy::Easy::new();
            easy.follow_location(true)?;
            easy.url(url)?;

            #[cfg(not(feature = "no_std_out"))]
            {
                use colored::*;

                let output = format!("{:>15} - Downloading \"{}\"", "Installation", url);
                println!("{}", output.dimmed());

                easy.progress(true)?;
                easy.progress_function(|total_download, current_download, _, _| {
                    let percent = (current_download / total_download) * 100.0;
                    let size_in_megabytes = total_download as u64 / 1_048_576;
                    let output = format!(
                        "\r{:>15} - {:.2}% complete ({:#} MB total)",
                        "Installation", percent, size_in_megabytes
                    );
                    print!("{}", output.dimmed());
                    true
                })?;
            }

            let mut transfer = easy.transfer();
            transfer.write_function(|data| {
                buffer.extend_from_slice(data);
                Ok(data.len())
            })?;
            Ok(transfer.perform()?)
        }

        #[cfg(feature = "wasm")]
        fn remote_fetch(
            buffer: alloc::sync::Weak<parking_lot::RwLock<Vec<u8>>>,
            url: &'static str,
        ) -> Result<(), $crate::errors::ParameterError> {
            // NOTE(julesdesmit): We spawn a local thread here in order to be
            // able to accommodate the async syntax from reqwest.
            wasm_bindgen_futures::spawn_local(async move {
                let content = reqwest::get(url).await.unwrap().text().await.unwrap();

                let buffer = buffer.upgrade().unwrap();
                buffer.write().extend_from_slice(content.as_bytes());
                drop(buffer);
            });
            Ok(())
        }
    };
}

macro_rules! impl_load_bytes_logic_local {
    ($buffer: expr, $expected_size: expr, $expected_checksum: expr) => {
        // Ensure the size matches.
        if $expected_size != $buffer.len() {
            return Err($crate::errors::ParameterError::SizeMismatch($expected_size, $buffer.len()));
        }

        // Ensure the checksum matches.
        let candidate_checksum = checksum!($buffer);
        if $expected_checksum != candidate_checksum {
            return checksum_error!($expected_checksum, candidate_checksum);
        }

        return Ok($buffer.to_vec());
    };
}

macro_rules! impl_load_bytes_logic_remote {
    ($remote_url: expr, $local_dir: expr, $filename: expr, $metadata: expr, $expected_checksum: expr, $expected_size: expr) => {
        // Compose the correct file path for the parameter file.
        let mut file_path = aleo_std::aleo_dir();
        file_path.push($local_dir);
        file_path.push($filename);

        let buffer = if file_path.exists() {
            // Attempts to load the parameter file locally with an absolute path.
            std::fs::read(file_path)?
        } else {
            // Downloads the missing parameters and stores it in the local directory for use.
             #[cfg(not(feature = "no_std_out"))]
            {
                use colored::*;
                let path = format!("(in {:?})", file_path);
                eprintln!(
                    "\n⚠️  \"{}\" does not exist. Downloading and storing it {}.\n",
                    $filename, path.dimmed()
                );
            }

            // Construct the URL.
            let url = format!("{}/{}", $remote_url, $filename);

            // Load remote file
            cfg_if::cfg_if! {
                if #[cfg(not(feature = "wasm"))] {
                    let mut buffer = vec![];
                    Self::remote_fetch(&mut buffer, &url)?;

                    // Ensure the checksum matches.
                    let candidate_checksum = checksum!(&buffer);
                    if $expected_checksum != candidate_checksum {
                        return checksum_error!($expected_checksum, candidate_checksum)
                    }

                    match Self::store_bytes(&buffer, &file_path) {
                        Ok(()) => buffer,
                        Err(_) => {
                            eprintln!(
                                "\n❗ Error - Failed to store \"{}\" locally. Please download this file manually and ensure it is stored in {:?}.\n",
                                $filename, file_path
                            );
                            buffer
                        }
                    }
                } else if #[cfg(feature = "wasm")] {
                    let buffer = alloc::sync::Arc::new(parking_lot::RwLock::new(vec![]));

                    // NOTE(julesdesmit): I'm leaking memory here so that I can get a
                    // static reference to the url, which is needed to pass it into
                    // the local thread which downloads the file.
                    let url = Box::leak(url.into_boxed_str());

                    let buffer_clone = alloc::sync::Arc::downgrade(&buffer);
                    Self::remote_fetch(buffer_clone, url)?;

                    // Recover the bytes.
                    let buffer = alloc::sync::Arc::try_unwrap(buffer).unwrap();
                    let buffer = buffer.write().clone();

                    // Ensure the checksum matches.
                    let candidate_checksum = checksum!(&buffer);
                    if $expected_checksum != candidate_checksum {
                        return checksum_error!($expected_checksum, candidate_checksum)
                    }

                    buffer
                } else {
                    return Err($crate::errors::ParameterError::RemoteFetchDisabled);
                }
            }
        };

        // Ensure the size matches.
        if $expected_size != buffer.len() {
            return Err($crate::errors::ParameterError::SizeMismatch($expected_size, buffer.len()));
        }

        // Ensure the checksum matches.
        let candidate_checksum = checksum!(buffer.as_slice());
        if $expected_checksum != candidate_checksum {
            return checksum_error!($expected_checksum, candidate_checksum)
        }

        return Ok(buffer)
    }
}

#[macro_export]
macro_rules! impl_local {
    ($name: ident, $local_dir: expr, $fname: tt, "usrs") => {
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct $name;

        impl $name {
            pub fn load_bytes() -> Result<Vec<u8>, $crate::errors::ParameterError> {
                const METADATA: &'static str = include_str!(concat!($local_dir, $fname, ".metadata"));

                let metadata: serde_json::Value =
                    serde_json::from_str(METADATA).expect("Metadata was not well-formatted");
                let expected_checksum: String =
                    metadata["checksum"].as_str().expect("Failed to parse checksum").to_string();
                let expected_size: usize =
                    metadata["size"].to_string().parse().expect("Failed to retrieve the file size");

                let buffer = include_bytes!(concat!($local_dir, $fname, ".", "usrs"));

                impl_load_bytes_logic_local!(buffer, expected_size, expected_checksum);
            }
        }

        paste::item! {
            #[cfg(test)]
            #[test]
            fn [< test_ $fname _usrs >]() {
                assert!($name::load_bytes().is_ok());
            }
        }
    };
    ($name: ident, $local_dir: expr, $fname: tt, $ftype: tt) => {
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct $name;

        impl $name {
            pub fn load_bytes() -> Result<Vec<u8>, $crate::errors::ParameterError> {
                const METADATA: &'static str = include_str!(concat!($local_dir, $fname, ".metadata"));

                let metadata: serde_json::Value =
                    serde_json::from_str(METADATA).expect("Metadata was not well-formatted");
                let expected_checksum: String =
                    metadata[concat!($ftype, "_checksum")].as_str().expect("Failed to parse checksum").to_string();
                let expected_size: usize =
                    metadata[concat!($ftype, "_size")].to_string().parse().expect("Failed to retrieve the file size");

                let buffer = include_bytes!(concat!($local_dir, $fname, ".", $ftype));

                impl_load_bytes_logic_local!(buffer, expected_size, expected_checksum);
            }
        }

        paste::item! {
            #[cfg(test)]
            #[test]
            fn [< test_ $fname _ $ftype >]() {
                assert!($name::load_bytes().is_ok());
            }
        }
    };
}

#[macro_export]
macro_rules! impl_remote {
    ($name: ident, $remote_url: expr, $local_dir: expr, $fname: tt, "usrs") => {
        pub struct $name;

        impl $name {
            impl_store_and_remote_fetch!();

            pub fn load_bytes() -> Result<Vec<u8>, $crate::errors::ParameterError> {
                const METADATA: &'static str = include_str!(concat!($local_dir, $fname, ".metadata"));

                let metadata: serde_json::Value =
                    serde_json::from_str(METADATA).expect("Metadata was not well-formatted");
                let expected_checksum: String =
                    metadata["checksum"].as_str().expect("Failed to parse checksum").to_string();
                let expected_size: usize =
                    metadata["size"].to_string().parse().expect("Failed to retrieve the file size");

                // Construct the versioned filename.
                let filename = match expected_checksum.get(0..7) {
                    Some(sum) => format!("{}.{}.{}", $fname, "usrs", sum),
                    _ => format!("{}.{}", $fname, "usrs"),
                };

                impl_load_bytes_logic_remote!(
                    $remote_url,
                    $local_dir,
                    &filename,
                    metadata,
                    expected_checksum,
                    expected_size
                );
            }
        }
        paste::item! {
            #[cfg(test)]
            #[test]
            fn [< test_ $fname _usrs >]() {
                assert!($name::load_bytes().is_ok());
            }
        }
    };
    ($name: ident, $remote_url: expr, $local_dir: expr, $fname: tt, $ftype: tt) => {
        pub struct $name;

        impl $name {
            impl_store_and_remote_fetch!();

            pub fn load_bytes() -> Result<Vec<u8>, $crate::errors::ParameterError> {
                const METADATA: &'static str = include_str!(concat!($local_dir, $fname, ".metadata"));

                let metadata: serde_json::Value =
                    serde_json::from_str(METADATA).expect("Metadata was not well-formatted");
                let expected_checksum: String =
                    metadata[concat!($ftype, "_checksum")].as_str().expect("Failed to parse checksum").to_string();
                let expected_size: usize =
                    metadata[concat!($ftype, "_size")].to_string().parse().expect("Failed to retrieve the file size");

                // Construct the versioned filename.
                let filename = match expected_checksum.get(0..7) {
                    Some(sum) => format!("{}.{}.{}", $fname, $ftype, sum),
                    _ => format!("{}.{}", $fname, $ftype),
                };

                impl_load_bytes_logic_remote!(
                    $remote_url,
                    $local_dir,
                    &filename,
                    metadata,
                    expected_checksum,
                    expected_size
                );
            }
        }

        paste::item! {
            #[cfg(test)]
            #[test]
            fn [< test_ $fname _ $ftype >]() {
                assert!($name::load_bytes().is_ok());
            }
        }
    };
}
