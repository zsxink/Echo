//! In-memory artwork adapter follows the same commit-before-projection boundary.
use super::*;
use crate::application::ports::ArtistCoverEditPort;
use crate::domain::library::durability::{ArtistCoverState, DurableResource, StateKind};
use crate::domain::library::PortableAssetReference;

impl ArtistCoverEditPort for MemoryDatabase {
    fn set_library_artist_cover(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        artist_key: &str,
        asset: Option<PortableAssetReference>,
        device: DeviceId,
    ) -> Result<(), Error> {
        let mut store = self.lock();
        if device != store.device_id {
            return Err(Error::conflict(
                "library edit author is not the local device",
            ));
        }
        let durable = control
            .durability()
            .ok_or_else(|| Error::unavailable("artist artwork", "directory adapter missing"))?;
        let replay = durable.recover(root)?;
        let manifest = control
            .read_manifest(root)?
            .ok_or_else(|| Error::unavailable("artist artwork", "library manifest missing"))?;
        let artwork = ArtistCoverState::new(manifest.library_id, artist_key.to_owned(), asset)?;
        let current = replay
            .states()
            .filter(|state| state.kind == StateKind::ArtistCover && state.id == artwork.id())
            .last()
            .map(ArtistCoverState::from_change)
            .transpose()?;
        if current.as_ref() == Some(&artwork) {
            let mut projected = store.clone();
            project_records(&mut projected, root, &replay);
            if self.fail_commit.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(Error::unavailable(
                    "test transaction",
                    "simulated commit failure",
                ));
            }
            *store = projected;
            return Ok(());
        }
        let mut batch = LibraryBatch::new(
            uuid::Uuid::new_v4(),
            manifest.library_id,
            ChangeOrigin::LocalEdit,
            vec![],
        );
        if let Some(asset) = &artwork.asset {
            batch.resources.push(DurableResource {
                relative_path: format!("echo/assets/{}", asset.content_hash),
                content_hash: asset.content_hash.clone(),
                byte_length: asset.byte_len,
            });
        }
        batch.state.push(artwork.into_change()?);
        commit_then_project(control, root, &batch, |_| {
            let replay = durable.recover(root)?;
            let mut projected = store.clone();
            project_records(&mut projected, root, &replay);
            if self.fail_commit.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(Error::unavailable(
                    "test transaction",
                    "simulated commit failure",
                ));
            }
            *store = projected;
            Ok(())
        })?;
        Ok(())
    }
}
