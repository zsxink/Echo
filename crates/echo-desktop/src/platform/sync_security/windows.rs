//! Safe Windows permissions and atomic-file adapters for the app-data vault.

use std::path::Path;

use echo_core::error::Error;
use windows_permissions::constants::{AccessRights, AceType, SeObjectType, SecurityInformation};
use windows_permissions::{utilities, wrappers, LocalBox, SecurityDescriptor};

use super::unavailable;

/// `TempPath`'s Windows adapter uses `MoveFileExW(REPLACE_EXISTING)` on this same
/// volume. Unlike remove-then-rename, a failed replacement retains the old slot.
pub(super) fn replace(temp: &Path, path: &Path) -> Result<(), Error> {
    tempfile::TempPath::try_from_path(temp)
        .and_then(|temp| temp.persist(path).map_err(|error| error.error))
        .map_err(|_| unavailable("local sync vault could not publish a credential"))
}

/// Install a protected DACL granting only the current process user access.
/// No permission prompt, identity string, or secret reaches the WebView/logs.
pub(super) fn restrict_to_current_user(path: &Path, directory: bool) -> Result<(), Error> {
    let user = utilities::current_process_sid()
        .map_err(|_| unavailable("local sync vault user identity is unavailable"))?;
    let user_string = wrappers::ConvertSidToStringSid(&user)
        .map_err(|_| unavailable("local sync vault user identity is unavailable"))?;
    let inheritance = if directory { "OICI" } else { "" };
    let descriptor: LocalBox<SecurityDescriptor> = format!(
        "D:P(A;{inheritance};FA;;;{})",
        user_string.to_string_lossy()
    )
    .parse()
    .map_err(|_| unavailable("local sync vault permissions are unavailable"))?;
    let acl = descriptor
        .dacl()
        .ok_or_else(|| unavailable("local sync vault permissions are unavailable"))?;
    wrappers::SetNamedSecurityInfo(
        path.as_os_str(),
        SeObjectType::SE_FILE_OBJECT,
        SecurityInformation::Dacl | SecurityInformation::ProtectedDacl,
        None,
        None,
        Some(acl),
        None,
    )
    .map_err(|_| unavailable("local sync vault permissions could not be set"))?;
    let persisted = wrappers::GetNamedSecurityInfo(
        path.as_os_str(),
        SeObjectType::SE_FILE_OBJECT,
        SecurityInformation::Dacl,
    )
    .map_err(|_| unavailable("local sync vault permissions could not be verified"))?;
    let persisted_acl = persisted
        .dacl()
        .ok_or_else(|| unavailable("local sync vault permissions are unsafe"))?;
    let only_user = persisted_acl.len() == 1
        && persisted_acl.get_ace(0).is_some_and(|entry| {
            entry.ace_type() == AceType::ACCESS_ALLOWED_ACE_TYPE
                && entry.sid() == Some(&*user)
                && entry.mask() == AccessRights::FileAllAccess
        });
    if !only_user {
        return Err(unavailable("local sync vault permissions are unsafe"));
    }
    Ok(())
}
