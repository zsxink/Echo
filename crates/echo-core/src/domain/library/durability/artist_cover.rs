//! Local artist artwork has a stable library/key identity and an original asset.
use super::{DurableStateChange, StateKind};
use crate::domain::library::{LibraryId, PortableAssetReference};
use crate::domain::text::normalized_key;
use crate::error::{Error, Subject};
use serde::{Deserialize, Serialize};

const SCHEMA: &str = "echo.library.artist-cover.v1";

/// Library-private artwork preference. It is never a LAN object or credential.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtistCoverState {
    schema: String,
    pub library_id: LibraryId,
    pub artist_key: String,
    /// An explicit absence clears the manual cover while retaining old bytes.
    pub asset: Option<PortableAssetReference>,
}

impl ArtistCoverState {
    /// # Errors
    /// Rejects noncanonical artist identities and invalid original references.
    pub fn new(
        library_id: LibraryId,
        artist_key: String,
        asset: Option<PortableAssetReference>,
    ) -> Result<Self, Error> {
        let state = Self {
            schema: SCHEMA.to_owned(),
            library_id,
            artist_key,
            asset,
        };
        state.validate()?;
        Ok(state)
    }

    /// # Errors
    /// Rejects identities that differ from the catalog's normalized artist key.
    pub fn validate_artist_key(key: &str) -> Result<(), Error> {
        if key.is_empty() || key.len() > 4096 || normalized_key(key) != key {
            return Err(Error::validation(
                Subject::Other,
                "artist key",
                "a canonical nonempty artist identity is required",
            ));
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), Error> {
        if self.schema != SCHEMA || self.library_id.as_uuid().is_nil() {
            return Err(Error::conflict("invalid artist artwork schema or library"));
        }
        Self::validate_artist_key(&self.artist_key)?;
        if let Some(asset) = &self.asset {
            asset.validate()?;
        }
        Ok(())
    }

    #[must_use]
    pub fn id(&self) -> uuid::Uuid {
        let mut hash = blake3::Hasher::new();
        hash.update(b"echo.library.artist-cover.v1\0");
        hash.update(self.library_id.as_uuid().as_bytes());
        hash.update(self.artist_key.as_bytes());
        let mut bytes = [0; 16];
        bytes.copy_from_slice(&hash.finalize().as_bytes()[..16]);
        uuid::Uuid::from_bytes(bytes)
    }

    /// # Errors
    /// Reports serialization failure; validation is performed before encoding.
    pub fn into_change(self) -> Result<DurableStateChange, Error> {
        self.validate()?;
        Ok(DurableStateChange::new(
            StateKind::ArtistCover,
            self.id(),
            serde_json::to_value(self).map_err(super::state::encode)?,
        ))
    }

    /// # Errors
    /// Rejects unknown/malformed schemas, scoped substitutions and changed keys.
    pub fn from_change(change: &DurableStateChange) -> Result<Self, Error> {
        if change.kind != StateKind::ArtistCover || change.key.is_some() {
            return Err(Error::conflict("artist artwork requires local typed state"));
        }
        let state: Self =
            serde_json::from_value(change.payload.clone()).map_err(super::state::encode)?;
        state.validate()?;
        if state.id() != change.id {
            return Err(Error::conflict(
                "artist artwork identity differs from its key",
            ));
        }
        Ok(state)
    }
}
