use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

pub fn same_file(left: &Path, right: &Path) -> io::Result<bool> {
    same_file::is_same_file(left, right)
}

#[derive(Debug)]
pub struct Directory {
    directory: Dir,
    path: PathBuf,
    identity: same_file::Handle,
}
impl Directory {
    pub fn open(path: &Path) -> io::Result<Self> {
        if fs::symlink_metadata(path)?.file_type().is_symlink() {
            return Err(io::Error::other("symlinked directory"));
        }
        let directory = Dir::open_ambient_dir(path, cap_std::ambient_authority())?;
        let identity = same_file::Handle::from_file(directory.try_clone()?.into_std_file())?;
        let result = Self {
            directory,
            path: path.into(),
            identity,
        };
        result.check()?;
        Ok(result)
    }
    pub fn check(&self) -> io::Result<()> {
        if fs::symlink_metadata(&self.path)?.file_type().is_symlink()
            || same_file::Handle::from_path(&self.path)? != self.identity
        {
            return Err(io::Error::other("directory identity changed"));
        }
        Ok(())
    }
    pub fn dir(&self) -> io::Result<&Dir> {
        self.check()?;
        Ok(&self.directory)
    }
    pub fn subdir(&self, path: &Path) -> io::Result<Dir> {
        self.parent(&path.join(".unused")).map(|(parent, _)| parent)
    }
    pub fn ensure_dir(&self, path: &Path) -> io::Result<()> {
        self.check()?;
        let mut parent = self.directory.try_clone()?;
        for component in path.components() {
            let Component::Normal(part) = component else {
                return Err(io::Error::other("invalid directory path"));
            };
            match parent.symlink_metadata(part) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => (),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    let builder = cap_std::fs::DirBuilder::new();
                    #[cfg(unix)]
                    let mut builder = builder;
                    #[cfg(unix)]
                    {
                        use cap_std::fs::DirBuilderExt;
                        builder.mode(0o700);
                    }
                    parent.create_dir_with(part, &builder)?;
                }
                _ => return Err(io::Error::other("unsafe directory")),
            }
            parent = parent.open_dir_nofollow(part)?;
        }
        Ok(())
    }
    fn parent(&self, path: &Path) -> io::Result<(Dir, std::ffi::OsString)> {
        self.check()?;
        let mut parts = Vec::new();
        for component in path.components() {
            if let Component::Normal(part) = component {
                parts.push(part);
            } else {
                return Err(io::Error::other("invalid capability path"));
            }
        }
        let name = parts
            .pop()
            .ok_or_else(|| io::Error::other("file name required"))?;
        let mut parent = self.directory.try_clone()?;
        for part in parts {
            if parent.symlink_metadata(part)?.file_type().is_symlink() {
                return Err(io::Error::other("symlinked ancestor"));
            }
            parent = parent.open_dir_nofollow(part)?;
        }
        Ok((parent, name.into()))
    }
    pub fn read(&self, path: &Path, maximum: u64, secret: bool) -> io::Result<Vec<u8>> {
        let (parent, name) = self.parent(path)?;
        if parent.symlink_metadata(&name)?.file_type().is_symlink() {
            return Err(io::Error::other("symlinked file"));
        }
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.custom_flags(
                (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
            );
        }
        let file = parent.open_with(&name, &options)?.into_std();
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > maximum {
            return Err(io::Error::other("invalid or oversized file"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & if secret { 0o077 } else { 0o022 } != 0 {
                return Err(io::Error::other("unsafe file permissions"));
            }
        }
        #[cfg(not(unix))]
        let _ = secret;
        let mut bytes = Vec::new();
        file.take(maximum + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > maximum {
            return Err(io::Error::other("file exceeds limit"));
        }
        self.check()?;
        Ok(bytes)
    }
    pub fn write(
        &self,
        path: &Path,
        bytes: &[u8],
        create: bool,
        preserve_mode: bool,
    ) -> io::Result<()> {
        let (parent, name) = self.parent(path)?;
        let permissions = if preserve_mode {
            let mut options = OpenOptions::new();
            options.read(true).follow(FollowSymlinks::No);
            #[cfg(unix)]
            {
                use cap_std::fs::OpenOptionsExt;
                options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
            }
            let metadata = parent.open_with(&name, &options)?.into_std().metadata()?;
            if !metadata.is_file() {
                return Err(io::Error::other("replace target is not a regular file"));
            }
            Some(metadata.permissions())
        } else {
            None
        };
        let temporary = format!(".llmup-{}.tmp", uuid::Uuid::new_v4());
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use cap_std::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = parent.open_with(&temporary, &options)?.into_std();
            file.write_all(bytes)?;
            if let Some(permissions) = permissions {
                file.set_permissions(permissions)?;
            }
            file.sync_all()?;
            self.check()?;
            if create {
                parent.hard_link(&temporary, &parent, &name)?;
                parent.remove_file(&temporary)?;
            } else {
                parent.rename(&temporary, &parent, &name)?;
            }
            #[cfg(unix)]
            parent.open(".")?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = parent.remove_file(&temporary);
        }
        result
    }
    pub fn remove(&self, path: &Path) -> io::Result<()> {
        let (parent, name) = self.parent(path)?;
        parent.remove_file(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn directory_handles_cannot_be_redirected_to_another_root() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir(home.path().join("root")).unwrap();
        fs::write(outside.path().join("secret"), "outside").unwrap();
        let directory = Directory::open(&home.path().join("root")).unwrap();
        fs::rename(home.path().join("root"), home.path().join("original")).unwrap();
        std::os::unix::fs::symlink(outside.path(), home.path().join("root")).unwrap();
        assert!(directory.read(Path::new("secret"), 1024, false).is_err());
        assert!(
            directory
                .write(Path::new("secret"), b"changed", false, false)
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(outside.path().join("secret")).unwrap(),
            "outside"
        );
    }
    #[test]
    fn writes_are_atomic_and_create_never_clobbers() {
        let home = tempfile::tempdir().unwrap();
        let directory = Directory::open(home.path()).unwrap();
        directory
            .write(Path::new("file"), b"first", true, false)
            .unwrap();
        assert!(
            directory
                .write(Path::new("file"), b"second", true, false)
                .is_err()
        );
        assert_eq!(
            directory.read(Path::new("file"), 10, true).unwrap(),
            b"first"
        );
        directory
            .write(Path::new("file"), b"second", false, true)
            .unwrap();
        assert_eq!(
            directory.read(Path::new("file"), 10, true).unwrap(),
            b"second"
        );
    }
}
