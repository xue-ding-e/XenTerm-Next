//! Private Windows CLI exports. The portable export key is public, so the
//! output must be treated as recoverable credentials, including while staging.
//!
//! Support is deliberately limited to local NTFS. Creation, inspection,
//! writing, rename, and failure cleanup retain the same file handle. No
//! pathname-based permission repair, replacement, or cleanup is permitted.

use super::{import, ConfigStore};
use anyhow::{bail, ensure, Context, Result};
use std::ffi::{c_void, OsStr, OsString};
use std::fs::File;
use std::io::Write;
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Component, Path, PathBuf, Prefix};
use windows::core::{Owned, PCWSTR, PWSTR};
use windows::Wdk::Storage::FileSystem::{
    FileRenameInformation, NtSetInformationFile, FILE_RENAME_INFORMATION,
};
use windows::Win32::Foundation::{
    BOOLEAN, ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_TOKEN,
    HANDLE, HLOCAL, STATUS_PENDING, STATUS_SUCCESS, WAIT_OBJECT_0,
};
use windows::Win32::Globalization::{CompareStringOrdinal, CSTR_EQUAL};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows::Win32::Security::{
    EqualSid, GetAce, GetSecurityDescriptorControl, GetSecurityDescriptorDacl,
    GetSecurityDescriptorOwner, GetTokenInformation, IsValidAcl, IsValidSecurityDescriptor,
    IsValidSid, TokenUser, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION,
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, SE_DACL_PRESENT,
    SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FileAttributeTagInfo, FileDispositionInfo, GetFileInformationByHandleEx,
    GetFileType, GetFinalPathNameByHandleW, GetVolumeInformationByHandleW,
    SetFileInformationByHandle, CREATE_NEW, FILE_ALL_ACCESS, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
    FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_READ_ATTRIBUTES, FILE_SHARE_MODE, FILE_SHARE_READ, FILE_TRAVERSE, FILE_TYPE_DISK,
    OPEN_EXISTING, VOLUME_NAME_GUID,
};
use windows::Win32::System::SystemServices::{ACCESS_ALLOWED_ACE_TYPE, FILE_PERSISTENT_ACLS};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken, WaitForSingleObject,
    INFINITE,
};
use windows::Win32::System::IO::IO_STATUS_BLOCK;

fn handle(file: &File) -> HANDLE {
    HANDLE(file.as_raw_handle())
}

fn wide(value: &OsStr) -> Result<Vec<u16>> {
    let mut value: Vec<u16> = value.encode_wide().collect();
    ensure!(!value.contains(&0), "export path contains a NUL character");
    value.push(0);
    Ok(value)
}

fn validate_local_path(path: &Path) -> Result<()> {
    ensure!(
        !path.as_os_str().is_empty() && !path.as_os_str().encode_wide().any(|c| c < 32 || c == 127),
        "export path must be nonempty and contain no control characters"
    );
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => ensure!(
                matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)),
                "private export requires a local disk path; network and device paths are unsupported"
            ),
            Component::Normal(name) => ensure!(
                !name.encode_wide().any(|c| c == b':' as u16),
                "alternate data streams are not valid export paths"
            ),
            _ => {}
        }
    }
    Ok(())
}

fn validate_filename(name: &OsStr) -> Result<()> {
    let name = name
        .to_str()
        .context("export filename must be valid Unicode")?;
    ensure!(
        !name.is_empty()
            && !name.ends_with(['.', ' '])
            && !name
                .chars()
                .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c)),
        "invalid export filename"
    );
    let stem = name.split('.').next().unwrap_or_default().to_uppercase();
    let reserved = matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|tail| {
            matches!(
                tail,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    });
    ensure!(!reserved, "device names are not valid export filenames");
    Ok(())
}

/// Volume GUID names resolve drive-letter, SUBST, junction and spelling aliases
/// into one local-volume namespace. Remote shares have no volume GUID path.
fn final_local_path(file: &File) -> Result<PathBuf> {
    let mut buffer = vec![0u16; 512];
    loop {
        let len = unsafe {
            GetFinalPathNameByHandleW(
                handle(file),
                &mut buffer,
                // FILE_NAME_NORMALIZED is zero, the default name form.
                VOLUME_NAME_GUID,
            )
        } as usize;
        if len == 0 {
            return Err(std::io::Error::last_os_error())
                .context("cannot verify a local export directory; network paths are unsupported");
        }
        if len < buffer.len() {
            buffer.truncate(len);
            let path = PathBuf::from(OsString::from_wide(&buffer));
            ensure!(
                path.as_os_str()
                    .to_string_lossy()
                    .starts_with("\\\\?\\Volume{"),
                "export destination is not a verified local volume"
            );
            return Ok(path);
        }
        ensure!(len <= 32768, "resolved export path is too long");
        buffer.resize(len + 1, 0);
    }
}

fn same_component(left: &OsStr, right: &OsStr) -> bool {
    let left: Vec<_> = left.encode_wide().collect();
    let right: Vec<_> = right.encode_wide().collect();
    // Conservatively treating case-sensitive siblings as equivalent can only
    // refuse an export; it cannot permit a case alias into the source profile.
    unsafe { CompareStringOrdinal(&left, &right, true) == CSTR_EQUAL }
}

fn within(directory: &Path, profile: &Path) -> bool {
    let mut directory = directory.components();
    profile.components().all(|part| {
        directory
            .next()
            .is_some_and(|candidate| same_component(candidate.as_os_str(), part.as_os_str()))
    })
}

fn verify_volume_properties(flags: u32, filesystem: &[u16]) -> Result<()> {
    let end = filesystem
        .iter()
        .position(|c| *c == 0)
        .unwrap_or(filesystem.len());
    ensure!(
        flags & FILE_PERSISTENT_ACLS != 0
            && filesystem[..end] == "NTFS".encode_utf16().collect::<Vec<_>>(),
        "private export requires a local NTFS volume that enforces persistent ACLs; no credentials were written"
    );
    Ok(())
}

fn verify_volume(file: &File) -> Result<()> {
    let mut flags = 0;
    let mut filesystem = [0u16; 32];
    unsafe {
        GetVolumeInformationByHandleW(
            handle(file),
            None,
            None,
            None,
            Some(&mut flags),
            Some(&mut filesystem),
        )
    }
    .context("cannot verify export filesystem permissions")?;
    verify_volume_properties(flags, &filesystem)
}

struct Directory {
    file: File,
    path: PathBuf,
}

impl Directory {
    fn open(path: &Path) -> Result<Self> {
        validate_local_path(path)?;
        let name = wide(path.as_os_str())?;
        // Deny delete/write sharing while the directory is pinned. This does
        // not prohibit attribute/reparse metadata changes; the created file's
        // resolved location is checked before any write. Native same-directory
        // rename uses the staging handle's parent without reopening a target
        // directory, so it does not need broader directory sharing.
        let raw = unsafe {
            CreateFileW(
                PCWSTR(name.as_ptr()),
                (FILE_TRAVERSE | FILE_READ_ATTRIBUTES).0,
                FILE_SHARE_READ,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        }
        .context("cannot open and protect export directory")?;
        let file = unsafe { File::from_raw_handle(raw.0) };
        ensure!(
            file.metadata()?.is_dir(),
            "export parent is not a directory"
        );
        let path = final_local_path(&file)?;
        Ok(Self { file, path })
    }
}

struct Descriptor(Owned<HLOCAL>);

impl Descriptor {
    fn parse(sddl: &str) -> Result<Self> {
        let text: Vec<_> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(text.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
        }?;
        Ok(Self(unsafe { Owned::new(HLOCAL(descriptor.0)) }))
    }

    fn pointer(&self) -> PSECURITY_DESCRIPTOR {
        PSECURITY_DESCRIPTOR((*self.0).0)
    }

    fn owner(&self) -> Result<PSID> {
        let mut owner = PSID::default();
        let mut defaulted = false.into();
        unsafe { GetSecurityDescriptorOwner(self.pointer(), &mut owner, &mut defaulted) }?;
        ensure!(
            !owner.0.is_null() && unsafe { IsValidSid(owner).as_bool() },
            "invalid export owner SID"
        );
        Ok(owner)
    }

    fn for_current_user() -> Result<Self> {
        let mut token = HANDLE::default();
        unsafe {
            if let Err(error) = OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut token) {
                if error.code() != ERROR_NO_TOKEN.to_hresult() {
                    return Err(error).context("cannot query effective export user");
                }
                OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
            }
        }
        let token = unsafe { Owned::new(token) };
        let mut length = 0;
        let query = unsafe { GetTokenInformation(*token, TokenUser, None, 0, &mut length) };
        ensure!(
            query.is_err_and(|error| error.code() == ERROR_INSUFFICIENT_BUFFER.to_hresult()),
            "cannot size export user token"
        );
        ensure!(
            length as usize >= size_of::<TOKEN_USER>() && length <= 65536,
            "invalid export user token length"
        );
        // TOKEN_USER contains a pointer; a byte Vec would not express its
        // required alignment. The SID points into this same live allocation.
        let mut storage = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        unsafe {
            GetTokenInformation(
                *token,
                TokenUser,
                Some(storage.as_mut_ptr().cast()),
                length,
                &mut length,
            )?;
        }
        let user = unsafe { &*storage.as_ptr().cast::<TOKEN_USER>() };
        ensure!(
            unsafe { IsValidSid(user.User.Sid).as_bool() },
            "invalid current-user SID"
        );
        let mut sid = PWSTR::null();
        unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid) }?;
        let _sid_allocation = unsafe { Owned::new(HLOCAL(sid.0.cast())) };
        let sid = unsafe { sid.to_string() }?;
        Self::parse(&format!("O:{sid}D:P(A;;FA;;;{sid})"))
    }
}

fn verify_descriptor(descriptor: &Descriptor, expected_owner: PSID) -> Result<()> {
    ensure!(
        unsafe { IsValidSecurityDescriptor(descriptor.pointer()).as_bool() },
        "invalid export security descriptor"
    );
    unsafe { EqualSid(descriptor.owner()?, expected_owner) }
        .context("export owner differs from the current user")?;
    let mut control = 0;
    let mut revision = 0;
    unsafe { GetSecurityDescriptorControl(descriptor.pointer(), &mut control, &mut revision) }?;
    ensure!(
        control & (SE_DACL_PRESENT.0 | SE_DACL_PROTECTED.0)
            == (SE_DACL_PRESENT.0 | SE_DACL_PROTECTED.0),
        "export DACL is absent or permits inheritance"
    );
    let mut present = false.into();
    let mut defaulted = false.into();
    let mut acl: *mut ACL = std::ptr::null_mut();
    unsafe {
        GetSecurityDescriptorDacl(descriptor.pointer(), &mut present, &mut acl, &mut defaulted)
    }?;
    ensure!(
        present.as_bool() && !defaulted.as_bool() && !acl.is_null(),
        "export has no explicit DACL"
    );
    ensure!(unsafe { IsValidAcl(acl).as_bool() }, "invalid export ACL");
    ensure!(
        unsafe { (*acl).AceCount } == 1,
        "export ACL grants access beyond its owner"
    );
    let mut ace: *mut c_void = std::ptr::null_mut();
    unsafe { GetAce(acl, 0, &mut ace) }?;
    // IsValidAcl validated the ACE layout. Check its type/size before reading
    // the variable-length SID at SidStart (not after ACCESS_ALLOWED_ACE).
    let header = unsafe { &*ace.cast::<ACE_HEADER>() };
    ensure!(
        header.AceType as u32 == ACCESS_ALLOWED_ACE_TYPE
            && header.AceSize as usize >= size_of::<ACCESS_ALLOWED_ACE>(),
        "invalid export ACE layout"
    );
    let ace = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
    ensure!(
        ace.Header.AceType as u32 == ACCESS_ALLOWED_ACE_TYPE
            && ace.Header.AceFlags == 0
            && ace.Header.AceSize as usize >= size_of::<ACCESS_ALLOWED_ACE>()
            && ace.Mask == FILE_ALL_ACCESS.0,
        "export ACL is not one explicit owner-only file-access grant"
    );
    let sid = PSID(std::ptr::addr_of!(ace.SidStart).cast_mut().cast());
    let sid_bytes = ace.Header.AceSize as usize - offset_of!(ACCESS_ALLOWED_ACE, SidStart);
    ensure!(sid_bytes >= 8, "truncated export ACL SID");
    let subauthorities = unsafe { sid.0.cast::<u8>().add(1).read() } as usize;
    ensure!(
        8 + subauthorities * 4 <= sid_bytes,
        "truncated export ACL SID"
    );
    ensure!(
        unsafe { IsValidSid(sid).as_bool() },
        "invalid export ACL SID"
    );
    unsafe { EqualSid(sid, expected_owner) }
        .context("export ACL grants access to another principal")?;
    Ok(())
}

fn verify_private_file(file: &File, owner: PSID) -> Result<()> {
    ensure!(
        unsafe { GetFileType(handle(file)) } == FILE_TYPE_DISK,
        "export is not a disk file"
    );
    let mut attributes = FILE_ATTRIBUTE_TAG_INFO::default();
    unsafe {
        GetFileInformationByHandleEx(
            handle(file),
            FileAttributeTagInfo,
            (&mut attributes as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    }?;
    ensure!(
        attributes.FileAttributes & (FILE_ATTRIBUTE_DIRECTORY.0 | FILE_ATTRIBUTE_REPARSE_POINT.0)
            == 0,
        "export is not a regular non-reparse file"
    );
    verify_volume(file)?;
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    unsafe {
        GetSecurityInfo(
            handle(file),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            None,
            None,
            None,
            None,
            Some(&mut descriptor),
        )
    }
    .ok()
    .context("cannot inspect effective export permissions")?;
    let descriptor = Descriptor(unsafe { Owned::new(HLOCAL(descriptor.0)) });
    verify_descriptor(&descriptor, owner)
}

struct StagedFile {
    file: File,
    finished: bool,
}

impl StagedFile {
    fn create(directory: &Directory, descriptor: &Descriptor) -> Result<Self> {
        let security = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.pointer().0,
            bInheritHandle: false.into(),
        };
        for _ in 0..16 {
            let path = directory
                .path
                .join(format!(".xenterm-export-{}", uuid::Uuid::new_v4()));
            let path = wide(path.as_os_str())?;
            let created = unsafe {
                CreateFileW(
                    PCWSTR(path.as_ptr()),
                    FILE_ALL_ACCESS.0,
                    FILE_SHARE_MODE(0),
                    Some(&security),
                    CREATE_NEW,
                    FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
                    None,
                )
            };
            match created {
                Ok(raw) => {
                    return Ok(Self {
                        file: unsafe { File::from_raw_handle(raw.0) },
                        finished: false,
                    })
                }
                Err(error)
                    if error.code() == ERROR_FILE_EXISTS.to_hresult()
                        || error.code() == ERROR_ALREADY_EXISTS.to_hresult() =>
                {
                    continue
                }
                Err(error) => {
                    return Err(error).context("cannot create private export staging file")
                }
            }
        }
        bail!("cannot allocate a unique private export staging file")
    }

    fn discard(&mut self) -> Result<()> {
        if !self.finished {
            let disposition = FILE_DISPOSITION_INFO {
                DeleteFile: BOOLEAN(1),
            };
            unsafe {
                SetFileInformationByHandle(
                    handle(&self.file),
                    FileDispositionInfo,
                    (&disposition as *const FILE_DISPOSITION_INFO).cast(),
                    size_of::<FILE_DISPOSITION_INFO>() as u32,
                )
            }
            .context("cannot remove private export staging file by handle")?;
            self.finished = true;
        }
        Ok(())
    }

    fn publish(&mut self, name: &OsStr) -> Result<()> {
        validate_filename(name)?;
        let name: Vec<u16> = name.encode_wide().collect();
        let filename_bytes = name
            .len()
            .checked_mul(2)
            .context("export filename is too long")?;
        // Follow the documented minimum: the complete fixed structure plus
        // the filename bytes, leaving its trailing padding/NUL space intact.
        let bytes = size_of::<FILE_RENAME_INFORMATION>()
            .checked_add(filename_bytes)
            .context("export filename is too long")?;
        // An aligned, zeroed allocation includes the variable-length filename.
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
        let mut completion = Box::new(IO_STATUS_BLOCK::default());
        completion.Anonymous.Status = STATUS_PENDING;
        let mut status = unsafe {
            (*info).Anonymous.ReplaceIfExists = BOOLEAN(0);
            (*info).RootDirectory = HANDLE::default();
            (*info).FileNameLength = u32::try_from(filename_bytes)?;
            std::ptr::copy_nonoverlapping(
                name.as_ptr(),
                std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
                name.len(),
            );
            // The native contract for NULL RootDirectory + a single leaf is
            // rename within the opened file's existing parent. It uses neither
            // the process cwd nor a new pathname lookup. Avoid the Win32
            // wrapper, which can expand relative names before forwarding them.
            // See Microsoft's FILE_RENAME_INFORMATION documentation and
            // [MS-FSA] 2.1.5.15.12 (DestinationDirectory = Open.Link.ParentFile).
            NtSetInformationFile(
                handle(&self.file),
                completion.as_mut(),
                info.cast(),
                u32::try_from(bytes)?,
                FileRenameInformation,
            )
        };
        if status == STATUS_PENDING {
            // CreateFileW did not request OVERLAPPED, but handle an unexpected
            // pending result without freeing buffers still owned by the I/O.
            let waited = unsafe { WaitForSingleObject(handle(&self.file), INFINITE) };
            if waited != WAIT_OBJECT_0 {
                std::mem::forget(storage);
                std::mem::forget(completion);
                bail!(
                    "cannot verify native export rename completion; retained pending I/O buffers"
                );
            }
            status = unsafe { completion.Anonymous.Status };
            if status == STATUS_PENDING {
                std::mem::forget(storage);
                std::mem::forget(completion);
                bail!(
                    "native export rename did not report completion; retained pending I/O buffers"
                );
            }
        }
        // For a non-pending return, the API return value is authoritative; for
        // a pending return, the completed I/O status above is authoritative.
        status
            .ok()
            .context("cannot publish export; destination must not already exist")?;
        ensure!(
            status == STATUS_SUCCESS,
            "native export rename returned an unexpected completion status"
        );
        self.finished = true;
        Ok(())
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        // Last-resort unwinding cleanup targets this handle, never a filename
        // that another process might have replaced. Normal errors call discard
        // explicitly so a cleanup error is returned to the caller.
        let _ = self.discard();
    }
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

pub(super) fn export(store: &ConfigStore, path: &Path) -> Result<usize> {
    validate_local_path(path)?;
    let name = path
        .file_name()
        .context("export destination must name a file")?;
    validate_filename(name)?;
    let destination = Directory::open(parent_directory(path))?;
    let source = Directory::open(parent_directory(&store.path))?;
    ensure!(
        !within(&destination.path, &source.path),
        "export destination must be outside the source profile directory"
    );
    verify_volume(&destination.file)?;
    let (raw, count) = store.export_json()?;
    ensure!(
        raw.len() <= import::MAX_IMPORT_BYTES,
        "portable export exceeds the 16 MiB import limit; no file was written"
    );
    let descriptor = Descriptor::for_current_user()?;
    let mut staged = StagedFile::create(&destination, &descriptor)?;
    let result = (|| -> Result<()> {
        verify_private_file(&staged.file, descriptor.owner()?)?;
        let actual = final_local_path(&staged.file)?;
        ensure!(
            actual
                .parent()
                .is_some_and(|p| within(p, &destination.path) && within(&destination.path, p)),
            "export staging directory changed before verification"
        );
        ensure!(
            !within(&actual, &final_local_path(&source.file)?),
            "export destination moved inside the source profile"
        );
        // No credential bytes reach any file before all handle-based checks.
        staged
            .file
            .write_all(raw.as_bytes())
            .context("cannot write portable export")?;
        staged
            .file
            .sync_all()
            .context("cannot flush portable export")?;
        staged.publish(name)
    })();
    if let Err(error) = result {
        staged
            .discard()
            .context(format!("export failed ({error:#}); cleanup also failed"))?;
        return Err(error);
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::super::{tests::temp_store, Secret, Session};
    use super::*;
    use std::fs;
    use windows::Win32::Security::{SetFileSecurityW, PROTECTED_DACL_SECURITY_INFORMATION};

    fn set_fixture_acl(path: &Path, sddl: &str) {
        let descriptor = Descriptor::parse(sddl).unwrap();
        let path = wide(path.as_os_str()).unwrap();
        unsafe {
            SetFileSecurityW(
                PCWSTR(path.as_ptr()),
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                descriptor.pointer(),
            )
        }
        .ok()
        .unwrap();
    }

    fn owner_string(descriptor: &Descriptor) -> String {
        let mut text = PWSTR::null();
        unsafe { ConvertSidToStringSidW(descriptor.owner().unwrap(), &mut text) }.unwrap();
        let _allocation = unsafe { Owned::new(HLOCAL(text.0.cast())) };
        unsafe { text.to_string() }.unwrap()
    }

    fn store(profile: &Path) -> ConfigStore {
        let mut store = temp_store();
        store.path = profile.join("sessions.db");
        let mut session = Session::new_empty();
        session.password = Secret::new("synthetic-export-password".to_owned());
        store.cache.sessions.push(session);
        store
    }

    #[test]
    fn windows_private_export_preserves_owner_only_acl_under_shared_parent() {
        let directory = tempfile::tempdir().unwrap();
        let profile = tempfile::tempdir().unwrap();
        set_fixture_acl(directory.path(), "D:P(A;OICI;FA;;;WD)");
        let output = directory.path().join("synthetic.json");
        assert_eq!(store(profile.path()).export_to_new(&output).unwrap(), 1);
        let descriptor = Descriptor::for_current_user().unwrap();
        verify_private_file(&File::open(&output).unwrap(), descriptor.owner().unwrap()).unwrap();
        let content = fs::read_to_string(&output).unwrap();
        assert!(content.contains("meatshell_export"));
        assert!(!content.contains("synthetic-export-password"));
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn windows_private_export_never_replaces_files_directories_or_hardlinks() {
        let directory = tempfile::tempdir().unwrap();
        let profile = tempfile::tempdir().unwrap();
        let source = profile.path().join("synthetic-key");
        fs::write(&source, "synthetic-source").unwrap();
        let file = directory.path().join("existing.json");
        fs::write(&file, "unchanged").unwrap();
        let link = directory.path().join("existing-link.json");
        fs::hard_link(&source, &link).unwrap();
        let folder = directory.path().join("existing-folder.json");
        fs::create_dir(&folder).unwrap();
        for output in [&file, &link, &folder] {
            assert!(store(profile.path()).export_to_new(output).is_err());
        }
        assert_eq!(fs::read_to_string(file).unwrap(), "unchanged");
        assert_eq!(fs::read_to_string(source).unwrap(), "synthetic-source");
        assert_eq!(fs::read_to_string(link).unwrap(), "synthetic-source");
        assert!(folder.is_dir());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 3);
    }

    #[test]
    fn windows_private_staging_is_exclusive_and_cleans_empty_file_by_handle() {
        let directory = tempfile::tempdir().unwrap();
        let parent = Directory::open(directory.path()).unwrap();
        let descriptor = Descriptor::for_current_user().unwrap();
        let mut staged = StagedFile::create(&parent, &descriptor).unwrap();
        verify_private_file(&staged.file, descriptor.owner().unwrap()).unwrap();
        assert_eq!(staged.file.metadata().unwrap().len(), 0);
        let path = fs::read_dir(directory.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert!(fs::rename(&path, directory.path().join("substituted")).is_err());
        assert!(fs::remove_file(&path).is_err());
        staged.discard().unwrap();
        drop(staged);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn windows_export_pins_empty_source_and_destination_directories() {
        let root = tempfile::tempdir().unwrap();
        // Test empty directories: protection must come from the directory
        // handles, not merely from the separately exclusive staging file.
        for name in ["source", "destination"] {
            let original = root.path().join(name);
            let moved = root.path().join(format!("{name}-moved"));
            fs::create_dir(&original).unwrap();
            let pinned = Directory::open(&original).unwrap();
            assert!(fs::rename(&original, &moved).is_err());
            assert!(fs::remove_dir(&original).is_err());
            assert!(original.is_dir());
            assert!(!moved.exists());
            drop(pinned);
            fs::rename(&original, &moved).unwrap();
            assert!(!original.exists());
            assert!(moved.is_dir());
        }
    }

    #[test]
    fn windows_private_verifier_rejects_broader_or_inheritable_descriptors() {
        let expected = Descriptor::for_current_user().unwrap();
        let sid = owner_string(&expected);
        for acl in [
            "D:P",
            "D:NO_ACCESS_CONTROL",
            "D:P(A;;FA;;;WD)",
            &format!("D:(A;;FA;;;{sid})"),
            &format!("D:P(A;;FA;;;{sid})(A;;FR;;;WD)"),
            &format!("D:P(A;ID;FA;;;{sid})"),
        ] {
            let candidate = Descriptor::parse(&format!("O:{sid}{acl}")).unwrap();
            assert!(
                verify_descriptor(&candidate, expected.owner().unwrap()).is_err(),
                "accepted {acl}"
            );
        }
        let wrong_owner = Descriptor::parse(&format!("O:WDD:P(A;;FA;;;{sid})")).unwrap();
        assert!(verify_descriptor(&wrong_owner, expected.owner().unwrap()).is_err());
    }

    #[test]
    fn windows_private_export_unsupported_volume_is_rejected_before_writing() {
        let directory = tempfile::tempdir().unwrap();
        let parent = Directory::open(directory.path()).unwrap();
        let descriptor = Descriptor::for_current_user().unwrap();
        let mut staged = StagedFile::create(&parent, &descriptor).unwrap();
        assert!(verify_volume_properties(0, &"NTFS".encode_utf16().collect::<Vec<_>>()).is_err());
        assert!(verify_volume_properties(
            FILE_PERSISTENT_ACLS,
            &"exFAT".encode_utf16().collect::<Vec<_>>()
        )
        .is_err());
        assert_eq!(staged.file.metadata().unwrap().len(), 0);
        staged.discard().unwrap();
        drop(staged);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn windows_private_file_verification_rejects_shared_acl_before_any_write() {
        let directory = tempfile::tempdir().unwrap();
        let parent = Directory::open(directory.path()).unwrap();
        let expected = Descriptor::for_current_user().unwrap();
        let sid = owner_string(&expected);
        let shared = Descriptor::parse(&format!("O:{sid}D:P(A;;FA;;;{sid})(A;;FR;;;WD)")).unwrap();
        let mut staged = StagedFile::create(&parent, &shared).unwrap();
        assert!(verify_private_file(&staged.file, expected.owner().unwrap()).is_err());
        assert_eq!(staged.file.metadata().unwrap().len(), 0);
        staged.discard().unwrap();
        drop(staged);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn windows_private_export_concurrent_publish_has_one_complete_winner() {
        let directory = tempfile::tempdir().unwrap();
        let profile = tempfile::tempdir().unwrap();
        let output = directory.path().join("concurrent.json");
        let barrier = std::sync::Barrier::new(2);
        let outcomes = std::thread::scope(|scope| {
            let run = || {
                let store = store(profile.path());
                barrier.wait();
                store.export_to_new(&output).is_ok()
            };
            let first = scope.spawn(run);
            let second = scope.spawn(run);
            [first.join().unwrap(), second.join().unwrap()]
        });
        assert_eq!(outcomes.into_iter().filter(|success| *success).count(), 1);
        let exported: serde_json::Value =
            serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
        assert_eq!(exported["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn windows_private_export_accepts_short_and_unicode_filenames() {
        let directory = tempfile::tempdir().unwrap();
        let profile = tempfile::tempdir().unwrap();
        for name in ["a", "合成凭据导出.json"] {
            let output = directory.path().join(name);
            assert_eq!(store(profile.path()).export_to_new(&output).unwrap(), 1);
            assert!(output.is_file());
        }
    }

    #[test]
    fn windows_native_publish_uses_the_staging_parent_not_the_process_cwd() {
        let directory = tempfile::tempdir().unwrap();
        let cwd = std::env::current_dir().unwrap();
        assert_ne!(
            directory.path().canonicalize().unwrap(),
            cwd.canonicalize().unwrap()
        );
        let parent = Directory::open(directory.path()).unwrap();
        let descriptor = Descriptor::for_current_user().unwrap();
        let mut staged = StagedFile::create(&parent, &descriptor).unwrap();
        verify_private_file(&staged.file, descriptor.owner().unwrap()).unwrap();
        staged.file.write_all(b"synthetic-native-rename").unwrap();
        staged.file.sync_all().unwrap();
        let leaf = format!("xenterm-native-{}.json", uuid::Uuid::new_v4());
        assert!(!cwd.join(&leaf).exists());
        staged.publish(OsStr::new(&leaf)).unwrap();
        drop(staged);
        assert_eq!(
            fs::read(directory.path().join(&leaf)).unwrap(),
            b"synthetic-native-rename"
        );
        assert!(!cwd.join(&leaf).exists());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn windows_private_export_rejects_profile_and_ancestor_junction_aliases() {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("profile");
        fs::create_dir(&profile).unwrap();
        for (alias, target, output) in [
            (
                root.path().join("profile-alias"),
                profile.clone(),
                "new.json",
            ),
            (
                root.path().join("ancestor-alias"),
                root.path().to_path_buf(),
                "profile/new.json",
            ),
        ] {
            // Creating an NTFS junction does not require the symbolic-link
            // privilege. Pass paths as data, never interpolate them into code.
            let result = std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command",
                    "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:XENTERM_TEST_ALIAS -Target $env:XENTERM_TEST_TARGET | Out-Null"])
                .env("XENTERM_TEST_ALIAS", &alias)
                .env("XENTERM_TEST_TARGET", &target)
                .output().unwrap();
            assert!(
                result.status.success(),
                "cannot create synthetic junction: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert!(store(&profile).export_to_new(&alias.join(output)).is_err());
            assert!(!profile.join("new.json").exists());
            fs::remove_dir(&alias).unwrap();
        }
    }

    #[test]
    fn windows_private_export_rejects_profile_case_aliases_ads_and_devices() {
        let profile = tempfile::tempdir().unwrap();
        fs::create_dir(profile.path().join("nested")).unwrap();
        let store = store(profile.path());
        for output in [
            profile.path().join("new.json"),
            profile.path().join("nested/new.json"),
            PathBuf::from(profile.path().to_str().unwrap().to_uppercase()).join("case.json"),
        ] {
            assert!(store.export_to_new(&output).is_err());
            assert!(!output.exists());
        }
        for path in [
            r"\\server\share\export.json",
            r"\\?\UNC\server\share\export.json",
            r"\\.\NUL",
            r"C:\file.json:secret",
        ] {
            assert!(validate_local_path(Path::new(path)).is_err());
        }
        for name in [
            "NUL",
            "CON.json",
            "CONIN$",
            "CONOUT$",
            "COM1",
            "LPT9.txt",
            "file.json:secret",
            "trailing.",
        ] {
            assert!(validate_filename(OsStr::new(name)).is_err());
        }
        assert!(!within(
            Path::new(r"C:\profile-backup"),
            Path::new(r"C:\profile")
        ));
        assert!(within(
            Path::new(r"C:\PROFILE\nested"),
            Path::new(r"c:\profile")
        ));
    }
}
