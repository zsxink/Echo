//! Lyrics candidates and source priority.

// ---------------------------------------------------------------------------
// Lyrics
// ---------------------------------------------------------------------------

/// Priority of a lyrics source. Total order: `Override > Embedded > Sidecar`.
///
/// The override semantics are subtle: an override that exists but is empty is
/// a *user clearing*, which must NOT fall through to a lower-priority source.
///
/// `Ord` is implemented manually because the derived variant order would be
/// the declaration order (Sidecar first), the opposite of the priority meaning.
impl Ord for LyricsSource {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.priority().cmp(&other.priority())
    }
}

impl PartialOrd for LyricsSource {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl LyricsSource {
    /// Numeric priority: `Override` = 3, `Embedded` = 2, `Sidecar` = 1.
    #[must_use]
    const fn priority(self) -> u8 {
        match self {
            Self::Override => 3,
            Self::Embedded => 2,
            Self::Sidecar => 1,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LyricsSource {
    /// The user-edited Echo override layer.
    Override,
    /// Embedded in the audio file (e.g. USLT/lyrics tag).
    #[default]
    Embedded,
    /// A same-basename `.lrc` sidecar in the file's directory.
    Sidecar,
}

/// A parsed lyrics candidate from one source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LyricsCandidate {
    source: LyricsSource,
    /// Timestamped lines, sorted by time. Empty for plain-text lyrics.
    lines: Vec<LyricsLine>,
    /// Original source text, retained for diagnostics and plain-text display.
    raw_text: String,
    /// Plain text content when no usable timestamps were found.
    plain_text: Option<String>,
    /// A non-fatal parse diagnostic. A malformed candidate must not prevent
    /// the audio record from being imported.
    parse_error: Option<String>,
    /// Whether the text is deliberately empty (a user cleared it). When set,
    /// lower-priority sources must NOT be considered.
    empty_override: bool,
}

/// One timestamped lyrics line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LyricsLine {
    /// Time in milliseconds.
    pub timestamp_ms: i64,
    pub text: String,
    /// Position in the source before timestamp sorting.
    pub original_index: usize,
}

impl LyricsCandidate {
    #[must_use]
    pub fn new(source: LyricsSource, lines: Vec<LyricsLine>, plain_text: bool) -> Self {
        let plain_text = plain_text.then(|| {
            lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        });
        Self::with_raw_text(source, String::new(), lines, plain_text, None)
    }

    /// Construct a candidate while retaining the parser input and any
    /// recoverable parse diagnostic.
    #[must_use]
    pub const fn with_raw_text(
        source: LyricsSource,
        raw_text: String,
        lines: Vec<LyricsLine>,
        plain_text: Option<String>,
        parse_error: Option<String>,
    ) -> Self {
        Self {
            source,
            lines,
            raw_text,
            plain_text,
            parse_error,
            empty_override: false,
        }
    }

    #[must_use]
    pub const fn source(&self) -> LyricsSource {
        self.source
    }
    #[must_use]
    pub fn lines(&self) -> &[LyricsLine] {
        &self.lines
    }
    #[must_use]
    pub const fn is_plain_text(&self) -> bool {
        self.plain_text.is_some()
    }
    #[must_use]
    pub fn raw_text(&self) -> &str {
        &self.raw_text
    }
    #[must_use]
    pub fn plain_text(&self) -> Option<&str> {
        self.plain_text.as_deref()
    }
    #[must_use]
    pub fn parse_error(&self) -> Option<&str> {
        self.parse_error.as_deref()
    }
    #[must_use]
    pub const fn is_empty_override(&self) -> bool {
        self.empty_override
    }

    /// Mark this candidate as a deliberate empty override (user cleared the
    /// lyrics); blocks fallback.
    pub const fn mark_empty_override(&mut self) {
        self.empty_override = true;
    }

    /// Whether this candidate is *effective*: it parsed without a fatal error
    /// and carries readable content (timed lines, plain text, or a deliberate
    /// empty override).
    #[must_use]
    pub fn is_effective(&self) -> bool {
        if self.parse_error.is_some() {
            return false;
        }
        if self.empty_override {
            return true;
        }
        !self.lines.is_empty() || self.plain_text.as_deref().is_some_and(|t| !t.is_empty())
    }
}

/// Select the lyrics candidate Echo displays for a song (task 4.5).
///
/// Priority is `Override > Embedded > Sidecar` among *effective* candidates:
/// a corrupt (parse-errored) higher-priority source falls back to the next
/// valid one, while a deliberate empty override (`empty_override`) is a user
/// clearing that must NOT fall through to a lower-priority source. `None`
/// means "no lyrics state" with the per-source diagnostics kept alongside.
#[must_use]
pub fn select_effective_lyrics(candidates: &[LyricsCandidate]) -> Option<&LyricsCandidate> {
    let mut ordered: Vec<&LyricsCandidate> = candidates.iter().collect();
    ordered.sort_by_key(|candidate| std::cmp::Reverse(candidate.source()));
    for candidate in ordered {
        if candidate.is_empty_override() {
            return Some(candidate);
        }
        if candidate.is_effective() {
            return Some(candidate);
        }
    }
    None
}
