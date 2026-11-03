//! macOS Now Playing projection and `MediaPlayer` adapter.
//!
//! The pure projection types in this module keep filesystem paths and platform
//! objects out of the playback domain. The Objective-C bridge is confined to
//! the macOS shell and delegates every remote command to the existing sink.

#![allow(unsafe_code)]

use std::sync::{Arc, OnceLock};

use echo_core::application::ports::CoverCache;
use echo_core::domain::state::PlaybackState;
use echo_desktop::platform::status_menu::StatusMenuSink;
use echo_desktop::player::port::{PlayerCommand, PlayerSnapshot};

#[derive(Clone, Debug, PartialEq)]
pub struct NowPlayingTrack {
    pub entry_id: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<f64>,
    pub cover_key: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NowPlayingProjection {
    pub entry_id: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<f64>,
    pub position: Option<f64>,
    pub rate: f64,
    pub cover_key: Option<String>,
    pub artwork: Option<Arc<Vec<u8>>>,
}

#[must_use]
pub fn project(
    snapshot: &PlayerSnapshot,
    track: Option<&NowPlayingTrack>,
) -> Option<NowPlayingProjection> {
    let track = track?;
    if track.title.trim().is_empty() {
        return None;
    }
    if matches!(
        snapshot.state,
        PlaybackState::Stopped | PlaybackState::Ended | PlaybackState::Failed
    ) {
        return None;
    }
    let duration = finite_positive(track.duration.or(snapshot.duration));
    let position = clamp_position(snapshot.position, duration);
    let rate = match snapshot.state {
        PlaybackState::Playing => 1.0,
        _ => 0.0,
    };
    Some(NowPlayingProjection {
        entry_id: track.entry_id.clone(),
        title: track.title.clone(),
        artist: non_empty(track.artist.clone()),
        album: non_empty(track.album.clone()),
        duration,
        position,
        rate,
        cover_key: non_empty(track.cover_key.clone()),
        artwork: None,
    })
}

/// Whether a cached Now Playing artwork object may be reused for `cover_key`.
///
/// Only the artwork built for exactly the same cover may be reused: a
/// different cover needs its own decode, and a track without artwork must
/// not publish the previous cover's image.
#[must_use]
pub fn artwork_is_reusable(cached_key: Option<&str>, cover_key: Option<&str>) -> bool {
    matches!((cached_key, cover_key), (Some(cached), Some(next)) if cached == next)
}

/// The cover artwork published for the current track, cached per cover.
///
/// The Now Playing publication follows the snapshot cadence (10 Hz while
/// playback is active). Reading a cover from disk and decoding it into a
/// platform image costs megabytes per call, so the payload is read once per
/// cover and reused until the cover changes. Re-reading it on every frame is
/// what drove the main-process memory sawtooth of issue #112.
///
/// The cached outcome covers the "this cover has no image" case as well: an
/// absent cover is remembered, so a track without artwork does not re-read the
/// cache on every frame either.
#[derive(Debug, Default)]
pub struct ArtworkCache {
    cover_key: Option<String>,
    bytes: Option<Arc<Vec<u8>>>,
}

impl ArtworkCache {
    /// A cache that has not seen any cover yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The artwork bytes for `cover_key`, reading them at most once per cover.
    ///
    /// The size-capped detail tier is used on purpose: the Now Playing
    /// artwork is requested at 512×512, so decoding the multi-megabyte
    /// original buys nothing and costs the memory this cache avoids.
    ///
    /// A read failure is remembered as "no artwork for this cover" rather
    /// than retried on every frame: the failed read can be as expensive as
    /// the successful one (the original is read before it can be decoded),
    /// and the cover key changes with the track.
    pub fn load(
        &mut self,
        cache: &dyn CoverCache,
        cover_key: Option<&str>,
    ) -> Option<Arc<Vec<u8>>> {
        let Some(cover_key) = cover_key else {
            self.cover_key = None;
            self.bytes = None;
            return None;
        };
        if self.cover_key.as_deref() == Some(cover_key) {
            return self.bytes.clone();
        }
        let bytes = match cache.thumbnail(cover_key, true) {
            Ok(Some(bytes)) => Some(Arc::new(bytes)),
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(%error, "failed to load the now playing artwork");
                None
            }
        };
        self.cover_key = Some(cover_key.to_owned());
        self.bytes.clone_from(&bytes);
        bytes
    }
}

#[must_use]
fn finite_positive(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite() && *value > 0.0)
}

#[must_use]
fn clamp_position(position: Option<f64>, duration: Option<f64>) -> Option<f64> {
    let position = position.filter(|value| value.is_finite() && *value >= 0.0)?;
    Some(duration.map_or(position, |duration| position.min(duration)))
}

#[must_use]
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RemoteCommand {
    Play,
    Pause,
    Previous,
    Next,
}

impl RemoteCommand {
    const fn player_command(self) -> PlayerCommand {
        match self {
            Self::Play => PlayerCommand::Play,
            Self::Pause => PlayerCommand::Pause,
            Self::Previous => PlayerCommand::Previous,
            Self::Next => PlayerCommand::Next,
        }
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::{
        artwork_is_reusable, Arc, NowPlayingProjection, OnceLock, RemoteCommand, StatusMenuSink,
    };
    use block2::RcBlock;
    use objc2::{
        define_class, extern_class, extern_methods, msg_send,
        rc::{Allocated, Retained},
        runtime::{AnyObject, NSObject, NSObjectProtocol},
        sel, AnyThread, DefinedClass, MainThreadOnly,
    };
    use objc2_app_kit::NSImage;
    use objc2_foundation::{MainThreadMarker, NSData, NSDictionary, NSNumber, NSSize, NSString};
    use std::cell::RefCell;
    use std::ptr::NonNull;

    extern_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "MPMediaItemArtwork"]
        struct MPMediaItemArtwork;
    );

    impl MPMediaItemArtwork {
        extern_methods!(
            #[unsafe(method(initWithBoundsSize:requestHandler:))]
            #[allow(non_snake_case)]
            pub fn initWithBoundsSize_requestHandler(
                this: Allocated<Self>,
                size: NSSize,
                handler: &block2::DynBlock<dyn Fn(NSSize) -> NonNull<NSImage>>,
            ) -> Retained<Self>;
        );
    }

    const HANDLER_SUCCESS: isize = 0;
    const HANDLER_FAILED: isize = 200;
    static NOW_PLAYING: OnceLock<usize> = OnceLock::new();

    thread_local! {
        /// The artwork object published for the current cover.
        ///
        /// Building it decodes the cover into a bitmap, which costs
        /// megabytes. The publication follows the 10 Hz snapshot cadence, so
        /// the object is built once per cover and reused until the cover
        /// changes.
        static ARTWORK: RefCell<Option<(String, Retained<MPMediaItemArtwork>)>> =
            const { RefCell::new(None) };
    }

    // These are exported NSString *constants, not their symbol names. Passing
    // `NSString::from_str("MPMediaItemPropertyTitle")` creates an unrelated
    // dictionary key which MediaPlayer deliberately ignores.
    extern "C" {
        static MPMediaItemPropertyTitle: *const NSString;
        static MPMediaItemPropertyArtist: *const NSString;
        static MPMediaItemPropertyAlbumTitle: *const NSString;
        static MPMediaItemPropertyPlaybackDuration: *const NSString;
        static MPMediaItemPropertyArtwork: *const NSString;
        static MPNowPlayingInfoPropertyElapsedPlaybackTime: *const NSString;
        static MPNowPlayingInfoPropertyPlaybackRate: *const NSString;
    }

    struct RemoteTargetIvars {
        sink: Arc<dyn StatusMenuSink>,
    }

    define_class!(
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        #[ivars = RemoteTargetIvars]
        struct RemoteTarget;

        unsafe impl NSObjectProtocol for RemoteTarget {}

        impl RemoteTarget {
            #[unsafe(method(handlePlay:))]
            fn handle_play(&self, _event: &AnyObject) -> isize {
                dispatch(&*self.ivars().sink, RemoteCommand::Play)
            }

            #[unsafe(method(handlePause:))]
            fn handle_pause(&self, _event: &AnyObject) -> isize {
                dispatch(&*self.ivars().sink, RemoteCommand::Pause)
            }

            #[unsafe(method(handlePrevious:))]
            fn handle_previous(&self, _event: &AnyObject) -> isize {
                dispatch(&*self.ivars().sink, RemoteCommand::Previous)
            }

            #[unsafe(method(handleNext:))]
            fn handle_next(&self, _event: &AnyObject) -> isize {
                dispatch(&*self.ivars().sink, RemoteCommand::Next)
            }
        }
    );

    fn dispatch(sink: &dyn StatusMenuSink, command: RemoteCommand) -> isize {
        match sink.on_command(command.player_command()) {
            Ok(()) => HANDLER_SUCCESS,
            Err(_) => HANDLER_FAILED,
        }
    }

    pub fn install(sink: Arc<dyn StatusMenuSink>) -> Result<(), String> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| "macOS Now Playing must be installed on the main thread".to_owned())?;
        if NOW_PLAYING.get().is_some() {
            return Err("macOS Now Playing is already installed".to_owned());
        }
        let center_class = objc2::runtime::AnyClass::get(c"MPNowPlayingInfoCenter")
            .ok_or_else(|| "MediaPlayer framework is unavailable".to_owned())?;
        let command_center_class = objc2::runtime::AnyClass::get(c"MPRemoteCommandCenter")
            .ok_or_else(|| "MediaPlayer command center is unavailable".to_owned())?;
        let center: Retained<AnyObject> = unsafe { msg_send![center_class, defaultCenter] };
        let commands: Retained<AnyObject> =
            unsafe { msg_send![command_center_class, sharedCommandCenter] };
        let target = RemoteTarget::alloc(mtm).set_ivars(RemoteTargetIvars { sink });
        let target: Retained<RemoteTarget> = unsafe { msg_send![super(target), init] };
        add_handler(&commands, "playCommand", &target, sel!(handlePlay:));
        add_handler(&commands, "pauseCommand", &target, sel!(handlePause:));
        add_handler(
            &commands,
            "previousTrackCommand",
            &target,
            sel!(handlePrevious:),
        );
        add_handler(&commands, "nextTrackCommand", &target, sel!(handleNext:));
        let _: () = unsafe { msg_send![&*center, setNowPlayingInfo: None::<&AnyObject>] };
        NOW_PLAYING
            .set(std::ptr::from_ref::<AnyObject>(&*center) as usize)
            .map_err(|_| "macOS Now Playing is already installed".to_owned())?;
        // MPRemoteCommand does not retain targets. Keep the Objective-C target
        // alive for the application lifetime, exactly as the status-row bridge
        // keeps its AppKit view alive. A raw pointer would leave controls wired
        // to a deallocated object as soon as `install` returns.
        let _target: &'static mut Retained<RemoteTarget> = Box::leak(Box::new(target));
        Ok(())
    }

    fn add_handler(
        commands: &AnyObject,
        property: &str,
        target: &RemoteTarget,
        action: objc2::runtime::Sel,
    ) {
        let property = NSString::from_str(property);
        let command: Retained<AnyObject> = unsafe { msg_send![commands, valueForKey: &*property] };
        let _: Retained<AnyObject> =
            unsafe { msg_send![&*command, addTarget: target, action: action] };
    }

    pub fn clear() {
        ARTWORK.with(|slot| {
            *slot.borrow_mut() = None;
        });
        let Some(address) = NOW_PLAYING.get() else {
            return;
        };
        let center = unsafe { &*(*address as *const AnyObject) };
        let _: () = unsafe { msg_send![center, setNowPlayingInfo: None::<&AnyObject>] };
    }

    /// Decode `bytes` into a `MediaPlayer` artwork object.
    fn build_artwork(bytes: &[u8]) -> Option<Retained<MPMediaItemArtwork>> {
        let data = NSData::with_bytes(bytes);
        let image = NSImage::initWithData(NSImage::alloc(), &data)?;
        let image_ptr = NonNull::from(image.as_ref());
        let handler = RcBlock::new(move |_size: NSSize| {
            let _keep_alive = &image;
            image_ptr
        });
        // `publish` runs on the main thread, so the marker is always there;
        // a missing one means the caller is off-thread, in which case the
        // artwork is skipped instead of panicking.
        let mtm = MainThreadMarker::new()?;
        Some(MPMediaItemArtwork::initWithBoundsSize_requestHandler(
            MPMediaItemArtwork::alloc(mtm),
            NSSize::new(512.0, 512.0),
            &handler,
        ))
    }

    /// The artwork object for `cover_key`, decoded at most once per cover.
    fn artwork_for(
        cover_key: Option<&str>,
        bytes: Option<&[u8]>,
    ) -> Option<Retained<MPMediaItemArtwork>> {
        ARTWORK.with(|slot| {
            let mut slot = slot.borrow_mut();
            let (Some(cover_key), Some(bytes)) = (cover_key, bytes) else {
                *slot = None;
                return None;
            };
            if let Some((cached_key, artwork)) = slot.as_ref() {
                if artwork_is_reusable(Some(cached_key.as_str()), Some(cover_key)) {
                    return Some(artwork.clone());
                }
            }
            let artwork = build_artwork(bytes)?;
            *slot = Some((cover_key.to_owned(), artwork.clone()));
            Some(artwork)
        })
    }

    pub fn publish(projection: &NowPlayingProjection) {
        let Some(address) = NOW_PLAYING.get() else {
            return;
        };
        let center = unsafe { &*(*address as *const AnyObject) };
        // SAFETY: `build.rs` links MediaPlayer on macOS. These framework
        // constants are process-lifetime immutable NSString instances.
        let title_key = unsafe { &*MPMediaItemPropertyTitle };
        let artist_key = unsafe { &*MPMediaItemPropertyArtist };
        let album_key = unsafe { &*MPMediaItemPropertyAlbumTitle };
        let duration_key = unsafe { &*MPMediaItemPropertyPlaybackDuration };
        let elapsed_key = unsafe { &*MPNowPlayingInfoPropertyElapsedPlaybackTime };
        let rate_key = unsafe { &*MPNowPlayingInfoPropertyPlaybackRate };
        let title = NSString::from_str(&projection.title);
        let mut keys: Vec<&NSString> = vec![title_key];
        let mut values: Vec<Retained<AnyObject>> = vec![title.into()];
        if let Some(artist) = projection.artist.as_ref() {
            keys.push(artist_key);
            values.push(NSString::from_str(artist).into());
        }
        if let Some(album) = projection.album.as_ref() {
            keys.push(album_key);
            values.push(NSString::from_str(album).into());
        }
        if let Some(duration) = projection.duration {
            keys.push(duration_key);
            values.push(NSNumber::new_f64(duration).into());
        }
        if let Some(position) = projection.position {
            keys.push(elapsed_key);
            values.push(NSNumber::new_f64(position).into());
        }
        keys.push(rate_key);
        values.push(NSNumber::new_f64(projection.rate).into());
        if let Some(artwork) = artwork_for(
            projection.cover_key.as_deref(),
            projection.artwork.as_ref().map(|bytes| bytes.as_slice()),
        ) {
            // SAFETY: see the MediaPlayer key constants above.
            keys.push(unsafe { &*MPMediaItemPropertyArtwork });
            values.push(artwork.into());
        }
        let object_refs: Vec<&AnyObject> = values.iter().map(|value| &**value).collect();
        let info = NSDictionary::<NSString, AnyObject>::from_slices(&keys, &object_refs);
        let _: () = unsafe { msg_send![center, setNowPlayingInfo: &*info] };
        let state: isize = if projection.rate > 0.0 { 1 } else { 2 };
        let _: () = unsafe { msg_send![center, setPlaybackState: state] };
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn metadata_keys_are_media_player_constants_not_symbol_name_strings() {
            // This is the regression guard for the bug where the literal
            // symbol name made every metadata field invisible to macOS.
            let title = unsafe { &*MPMediaItemPropertyTitle }.to_string();
            let duration = unsafe { &*MPMediaItemPropertyPlaybackDuration }.to_string();
            let artwork = unsafe { &*MPMediaItemPropertyArtwork }.to_string();
            assert_ne!(title, "MPMediaItemPropertyTitle");
            assert_ne!(duration, "MPMediaItemPropertyPlaybackDuration");
            assert_ne!(artwork, "MPMediaItemPropertyArtwork");
        }
    }
}

#[cfg(target_os = "macos")]
pub use native::{clear, install, publish};

#[cfg(not(target_os = "macos"))]
pub fn install(_sink: Arc<dyn StatusMenuSink>) -> Result<(), String> {
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn clear() {}

#[cfg(not(target_os = "macos"))]
pub fn publish(_projection: &NowPlayingProjection) {}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    use echo_core::error::Error;

    /// A cover cache that records what the Now Playing path asks for.
    #[derive(Default)]
    struct FakeCoverCache {
        thumbnails: Mutex<Vec<(String, bool)>>,
        originals: Mutex<usize>,
        payload: Option<Vec<u8>>,
        failure: bool,
    }

    impl FakeCoverCache {
        fn with_payload(payload: Vec<u8>) -> Self {
            Self {
                payload: Some(payload),
                ..Self::default()
            }
        }

        fn failing() -> Self {
            Self {
                failure: true,
                ..Self::default()
            }
        }

        fn thumbnail_calls(&self) -> Vec<(String, bool)> {
            self.thumbnails
                .lock()
                .expect("fake cover cache lock")
                .clone()
        }

        fn original_calls(&self) -> usize {
            *self.originals.lock().expect("fake cover cache lock")
        }
    }

    impl CoverCache for FakeCoverCache {
        fn put(&self, _bytes: &[u8], _mime: &str) -> Result<String, Error> {
            Ok("fake-asset".to_owned())
        }

        fn get(&self, _asset_key: &str) -> Result<Option<Vec<u8>>, Error> {
            *self.originals.lock().expect("fake cover cache lock") += 1;
            Ok(self.payload.clone())
        }

        fn thumbnail(&self, asset_key: &str, detail: bool) -> Result<Option<Vec<u8>>, Error> {
            self.thumbnails
                .lock()
                .expect("fake cover cache lock")
                .push((asset_key.to_owned(), detail));
            if self.failure {
                return Err(Error::io(
                    "read cover",
                    std::io::Error::other("injected cover read failure"),
                    std::path::PathBuf::from("/fake/cover"),
                ));
            }
            Ok(self.payload.clone())
        }

        fn gc(&self, _referenced_keys: &[String]) -> Result<(), Error> {
            Ok(())
        }
    }

    fn snapshot(
        state: PlaybackState,
        position: Option<f64>,
        duration: Option<f64>,
    ) -> PlayerSnapshot {
        PlayerSnapshot {
            state,
            position,
            duration,
            ..PlayerSnapshot::default()
        }
    }

    fn track() -> NowPlayingTrack {
        NowPlayingTrack {
            entry_id: "entry-1".into(),
            title: "Song".into(),
            artist: Some("Artist".into()),
            album: Some("Album".into()),
            duration: Some(120.0),
            cover_key: Some("cv1-asset".into()),
        }
    }

    #[test]
    fn projection_uses_safe_optional_metadata_and_clamps_position() {
        let projection = project(
            &snapshot(PlaybackState::Playing, Some(150.0), Some(120.0)),
            Some(&track()),
        )
        .expect("current track should publish");
        assert_eq!(projection.title, "Song");
        assert_eq!(projection.position, Some(120.0));
        assert!((projection.rate - 1.0).abs() < f64::EPSILON);
        assert_eq!(projection.artist.as_deref(), Some("Artist"));
        assert_eq!(projection.cover_key.as_deref(), Some("cv1-asset"));
    }

    #[test]
    fn projection_omits_missing_fields_and_never_contains_path() {
        let mut current = track();
        current.artist = Some(" ".into());
        current.album = None;
        current.cover_key = None;
        current.title = "outside.flac".into();
        let projection = project(
            &snapshot(PlaybackState::Paused, Some(-1.0), None),
            Some(&current),
        )
        .expect("display name is valid");
        assert_eq!(projection.artist, None);
        assert_eq!(projection.album, None);
        assert_eq!(projection.cover_key, None);
        assert_eq!(projection.position, None);
        assert!(!format!("{projection:?}").contains("/Users/"));
    }

    #[test]
    fn projection_clears_without_current_track_or_title() {
        assert_eq!(
            project(&snapshot(PlaybackState::Stopped, None, None), None),
            None
        );
        let mut current = track();
        current.title.clear();
        assert_eq!(
            project(
                &snapshot(PlaybackState::Stopped, None, None),
                Some(&current)
            ),
            None
        );
    }

    #[test]
    fn remote_commands_map_to_one_existing_player_command() {
        assert_eq!(RemoteCommand::Play.player_command(), PlayerCommand::Play);
        assert_eq!(RemoteCommand::Pause.player_command(), PlayerCommand::Pause);
        assert_eq!(
            RemoteCommand::Previous.player_command(),
            PlayerCommand::Previous
        );
        assert_eq!(RemoteCommand::Next.player_command(), PlayerCommand::Next);
    }

    #[test]
    fn now_playing_artwork_is_read_once_per_cover() {
        let cache = FakeCoverCache::with_payload(vec![1, 2, 3]);
        let mut artwork = ArtworkCache::new();
        for _ in 0..10 {
            let loaded = artwork.load(&cache, Some("cover-a"));
            assert_eq!(
                loaded.map(|bytes| bytes.as_ref().clone()),
                Some(vec![1, 2, 3])
            );
        }
        assert_eq!(cache.thumbnail_calls(), vec![("cover-a".to_owned(), true)]);
        // The full-size original is never read: the Now Playing artwork is
        // requested at 512×512, so only the size-capped tier is needed.
        assert_eq!(cache.original_calls(), 0);
    }

    #[test]
    fn now_playing_artwork_is_read_again_when_the_cover_changes() {
        let cache = FakeCoverCache::with_payload(vec![1]);
        let mut artwork = ArtworkCache::new();
        artwork.load(&cache, Some("cover-a"));
        artwork.load(&cache, Some("cover-b"));
        assert_eq!(cache.thumbnail_calls().len(), 2);
    }

    #[test]
    fn now_playing_artwork_is_not_read_again_for_a_cover_without_an_image() {
        let cache = FakeCoverCache::default();
        let mut artwork = ArtworkCache::new();
        assert!(artwork.load(&cache, Some("cover-a")).is_none());
        assert!(artwork.load(&cache, Some("cover-a")).is_none());
        assert_eq!(cache.thumbnail_calls().len(), 1);
    }

    #[test]
    fn now_playing_artwork_is_not_read_again_after_a_failed_read() {
        let cache = FakeCoverCache::failing();
        let mut artwork = ArtworkCache::new();
        assert!(artwork.load(&cache, Some("cover-a")).is_none());
        assert!(artwork.load(&cache, Some("cover-a")).is_none());
        assert_eq!(cache.thumbnail_calls().len(), 1);
    }

    #[test]
    fn now_playing_artwork_is_dropped_when_the_track_has_no_cover() {
        let cache = FakeCoverCache::with_payload(vec![1]);
        let mut artwork = ArtworkCache::new();
        assert!(artwork.load(&cache, Some("cover-a")).is_some());
        assert!(artwork.load(&cache, None).is_none());
        assert!(artwork.load(&cache, Some("cover-b")).is_some());
        assert_eq!(cache.thumbnail_calls().len(), 2);
    }

    #[test]
    fn now_playing_artwork_object_is_reused_only_for_the_same_cover() {
        assert!(artwork_is_reusable(Some("cover-a"), Some("cover-a")));
        assert!(!artwork_is_reusable(Some("cover-a"), Some("cover-b")));
        assert!(!artwork_is_reusable(Some("cover-a"), None));
        assert!(!artwork_is_reusable(None, Some("cover-a")));
    }
}
