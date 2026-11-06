use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

fn path() -> Result<PathBuf, &'static str> {
    Ok(directories::ProjectDirs::from("", "", "webos-mcp")
        .ok_or("Local credential storage is unavailable")?
        .data_local_dir()
        .join("credentials.json"))
}

fn choice_path() -> Result<PathBuf, &'static str> {
    Ok(path()?.with_file_name("credential-storage"))
}

pub fn chosen() -> Result<bool, &'static str> {
    chosen_at(&choice_path()?)
}

fn chosen_at(path: &Path) -> Result<bool, &'static str> {
    match read_path(path)? {
        Some(value) if value == "local" => Ok(true),
        Some(_) => Err("Local credential storage selection is invalid"),
        None => Ok(false),
    }
}

pub fn remember_choice() -> Result<(), &'static str> {
    remember_choice_at(&choice_path()?)
}

fn remember_choice_at(path: &Path) -> Result<(), &'static str> {
    if chosen_at(path)? {
        return Ok(());
    }
    save_path(path, "local", false)
}

pub fn read() -> Result<Option<String>, &'static str> {
    read_path(&path()?)
}

fn read_path(path: &Path) -> Result<Option<String>, &'static str> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("Local credential file is unavailable"),
    };
    if !metadata.is_file() || metadata.len() > 64 * 1024 {
        return Err("Local credential file is invalid");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("Local credential file permissions are too open");
        }
    }
    fs::read_to_string(path)
        .map(Some)
        .map_err(|_| "Local credential file is unavailable")
}

pub fn save(contents: &str, replace: bool) -> Result<(), &'static str> {
    save_path(&path()?, contents, replace)
}

fn save_path(path: &Path, contents: &str, replace: bool) -> Result<(), &'static str> {
    let directory = path
        .parent()
        .ok_or("Local credential storage is unavailable")?;
    fs::create_dir_all(directory).map_err(|_| "Local credential storage is unavailable")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .map_err(|_| "Could not protect local credential directory")?;
    }
    let mut temporary = tempfile::NamedTempFile::new_in(directory)
        .map_err(|_| "Could not create local credential file")?;
    temporary
        .write_all(contents.as_bytes())
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|_| "Could not save local credentials")?;
    if replace {
        temporary
            .persist(path)
            .map_err(|_| "Could not save local credentials")?;
    } else {
        temporary
            .persist_noclobber(path)
            .map_err(|_| "Local credentials already exist or could not be saved")?;
    }
    #[cfg(unix)]
    fs::File::open(directory)
        .and_then(|f| f.sync_all())
        .map_err(|_| "Could not confirm local credential storage")?;
    Ok(())
}

pub fn remove() -> Result<(), &'static str> {
    match fs::remove_file(path()?) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("Could not remove local credential file"),
    }?;
    match fs::remove_file(choice_path()?) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("Could not remove local storage selection"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_file_is_private_persistent_and_never_overwritten_on_create() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("app/credentials.json");
        assert!(read_path(&path).unwrap().is_none());
        save_path(&path, "synthetic", false).unwrap();
        assert_eq!(read_path(&path).unwrap().as_deref(), Some("synthetic"));
        assert!(save_path(&path, "other", false).is_err());
        assert_eq!(read_path(&path).unwrap().as_deref(), Some("synthetic"));
        save_path(&path, "updated", true).unwrap();
        assert_eq!(read_path(&path).unwrap().as_deref(), Some("updated"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn selection_remains_local_if_credential_file_is_lost() {
        let directory = tempfile::tempdir().unwrap();
        let choice = directory.path().join("app/credential-storage");
        let credentials = directory.path().join("app/credentials.json");
        assert!(!chosen_at(&choice).unwrap());
        save_path(&credentials, "synthetic", false).unwrap();
        remember_choice_at(&choice).unwrap();
        fs::remove_file(&credentials).unwrap();
        assert!(chosen_at(&choice).unwrap());
        assert!(read_path(&credentials).unwrap().is_none());
    }
    #[cfg(unix)]
    #[test]
    fn rejects_exposed_file_or_symlink_without_returning_contents() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("credentials");
        fs::write(&path, "synthetic").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_path(&path).is_err());
        let link = directory.path().join("link");
        symlink(&path, &link).unwrap();
        assert!(read_path(&link).is_err());
    }
}
