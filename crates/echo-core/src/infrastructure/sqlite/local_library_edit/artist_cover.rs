//! Artist artwork remains local state while its original lives in the library.
use super::{ChangeOrigin, LibraryBatch, SqliteDatabase};
use crate::application::ports::{ArtistCoverEditPort, ControlPlanePort};
use crate::domain::ids::LibraryRootId;
use crate::domain::library::durability::{ArtistCoverState, DurableResource, StateKind};
use crate::domain::library::{DeviceId, PortableAssetReference};
use crate::error::Error;

impl ArtistCoverEditPort for SqliteDatabase {
    fn set_library_artist_cover(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        artist_key: &str,
        asset: Option<PortableAssetReference>,
        device: DeviceId,
    ) -> Result<(), Error> {
        let artist_key = artist_key.to_owned();
        self.plan_library_edit(control, root, device, move |_, _, library, _, baseline| {
            let artwork = ArtistCoverState::new(library, artist_key, asset)?;
            let mut batch = LibraryBatch::new(
                uuid::Uuid::new_v4(),
                library,
                ChangeOrigin::LocalEdit,
                vec![],
            );
            if baseline
                .states()
                .filter(|state| state.kind == StateKind::ArtistCover && state.id == artwork.id())
                .last()
                .map(ArtistCoverState::from_change)
                .transpose()?
                .as_ref()
                == Some(&artwork)
            {
                return Ok(batch);
            }
            if let Some(asset) = &artwork.asset {
                batch.resources.push(DurableResource {
                    relative_path: format!("echo/assets/{}", asset.content_hash),
                    content_hash: asset.content_hash.clone(),
                    byte_length: asset.byte_len,
                });
            }
            batch.state.push(artwork.into_change()?);
            Ok(batch)
        })
    }
}
