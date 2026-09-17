#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::Read;
    use std::sync::{Arc, Mutex};
    use super::*;
    use crate::application::ports::{
        FileMeta, ImportSource, ImportSourceInfo, LibraryFileSystem, OperationJournalRepository,
        OperationResourceKind, SidecarInfo, SongRepository as _, StagedCopy,
    };
    use crate::application::scan::ScanConfig;
    use crate::application::testing::clock::{FakeIdGenerator, ManualClock};
    use crate::application::testing::small_fakes::{
        FakeFileHasher, FakeLyricsParser, FakeMediaProbe, FakeMetadataReader, MemoryControlPlane,
        MemoryCoverCache,
    };
    use crate::application::testing::ScanFixture;
    use crate::application::testing::{FakeImportSources, FakeLibraryFileSystem, MemoryDatabase};
    use crate::domain::entities::{select_effective_lyrics, LyricsSource, Song};
    use crate::domain::ids::Revision;
    use crate::domain::library::{PortableRecord, MEDIA_ROOT};
    use crate::domain::media::{AudioFormat, ParsedMetadata};
    use crate::domain::state::OperationState;
    use crate::domain::text::{
        target_artist_component, target_file_stem, truncate_component_with_extension,
    };

    include!("tests/planning.rs");
    include!("tests/execution.rs");
    include!("tests/report.rs");
    include!("tests/batch.rs");
    include!("tests/lyrics.rs");
}
