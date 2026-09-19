//! macOS Now Playing projection and MediaPlayer adapter.
//!
//! The pure projection types in this module keep filesystem paths and platform
//! objects out of the playback domain. The Objective-C bridge is confined to
//! the macOS shell and delegates every remote command to the existing sink.

#![allow(unsafe_code)]

use std::sync::{Arc, OnceLock};

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
    pub artwork: Option<Vec<u8>>,
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

#[must_use]
fn finite_positive(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite() && *value > 0.0)
}

#[must_use]
fn clamp_position(position: Option<f64>, duration: Option<f64>) -> Option<f64> {
    let position = position.filter(|value| value.is_finite() && *value >= 0.0)?;
    Some(match duration {
        Some(duration) => position.min(duration),
        None => position,
    })
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
    use std::ffi::CStr;

    use super::*;
    use block2::RcBlock;
    use objc2::{
        define_class, extern_class, extern_methods, msg_send,
        rc::{Allocated, Retained},
        runtime::{AnyObject, NSObject, NSObjectProtocol},
        sel, AnyThread, DefinedClass, MainThreadOnly,
    };
    use objc2_app_kit::NSImage;
    use objc2_foundation::{MainThreadMarker, NSData, NSDictionary, NSNumber, NSSize, NSString};
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

    // These are exported NSString *constants, not their symbol names. Passing
    // `NSString::from_str("MPMediaItemPropertyTitle")` creates an unrelated
    // dictionary key which MediaPlayer deliberately ignores.
    unsafe extern "C" {
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
        let center_class = objc2::runtime::AnyClass::get(
            CStr::from_bytes_with_nul(b"MPNowPlayingInfoCenter\0").expect("static class name"),
        )
        .ok_or_else(|| "MediaPlayer framework is unavailable".to_owned())?;
        let command_center_class = objc2::runtime::AnyClass::get(
            CStr::from_bytes_with_nul(b"MPRemoteCommandCenter\0").expect("static class name"),
        )
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
        let command: Retained<AnyObject> =
            unsafe { msg_send![&*commands, valueForKey: &*property] };
        let _: Retained<AnyObject> =
            unsafe { msg_send![&*command, addTarget: target, action: action] };
    }

    pub fn clear() {
        let Some(address) = NOW_PLAYING.get() else {
            return;
        };
        let center = unsafe { &*(*address as *const AnyObject) };
        let _: () = unsafe { msg_send![center, setNowPlayingInfo: None::<&AnyObject>] };
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
        if let Some(bytes) = projection.artwork.as_ref() {
            let data = NSData::with_bytes(bytes);
            if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
                let image_ptr = NonNull::from(image.as_ref());
                let handler = RcBlock::new(move |_size: NSSize| {
                    let _keep_alive = &image;
                    image_ptr
                });
                let mtm = MainThreadMarker::new().expect("publish runs on main thread");
                let artwork = MPMediaItemArtwork::initWithBoundsSize_requestHandler(
                    MPMediaItemArtwork::alloc(mtm),
                    NSSize::new(512.0, 512.0),
                    &handler,
                );
                // SAFETY: see the MediaPlayer key constants above.
                keys.push(unsafe { &*MPMediaItemPropertyArtwork });
                values.push(artwork.into());
            }
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
        assert_eq!(projection.rate, 1.0);
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
}
