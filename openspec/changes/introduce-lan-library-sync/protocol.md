# LAN synchronization protocol v1

`echo-sync` owns versioned JSON DTOs. Core domain values are reused for stable
identity, while transport framing, pagination, and resource references remain
outside Core. Writes require exact protocol version 1. Unknown optional JSON
fields are ignored; unknown required enum variants and unsupported protocol
versions are rejected.

## Bounds

- Mutation JSON: at most 1 MiB.
- Commit JSON: at most 1 MiB + 64 KiB, reserving confirmation-envelope space
  so a maximum-size valid mutation remains transportable.
- JSON payload nesting: at most 64 containers.
- Commit/snapshot response JSON and mutation ACK: at most 16 MiB.
- Snapshot page: at most 512 objects, aliases and owned resource references
  combined. An empty library has one terminal empty page. The requested
  page size limits records; resource and byte budgets can produce smaller
  frozen pages. Tokens are exactly 32 lowercase hexadecimal characters.
- Resource paths: validated `media/...` library-relative paths, at most 4096
  bytes; absolute paths and control-surface staging paths are not accepted.
- Resource hash: lowercase 64-character BLAKE3 hexadecimal digest.
- Resource media type: 1–127 bytes.
- Resource chunks: at most 4 MiB (`MAX_RESOURCE_CHUNK_BYTES`).

The resource manifest describes the full immutable resource by hash, length,
media type, and safe relative path. Chunk transfer is separate from commit
ordering. The portable `echo/tmp/` staging tree is never referenced.

## Commit digest

For a commit, serialize the `CommitDto` body as compact serde JSON using the
declared struct field order, then calculate BLAKE3 over these concatenated
fields:

1. ASCII domain tag `echo.lan.commit.v1\0`;
2. protocol version as unsigned 16-bit big-endian;
3. commit sequence as unsigned 64-bit big-endian;
4. previous digest (32 bytes);
5. body byte length as unsigned 32-bit big-endian;
6. serialized commit body.

The resulting digest is not embedded in the hashed body. `commit-v1.json` is a
checked-in compatibility fixture parsed by the protocol tests.
The v1 fixture digest is `dc2bbbabce5cd3132ff411710a7350df06a736d1ca48cc1e1abe572f8977424b`.

## Identity rules

Every mutation carries `(library_id, authority_id, epoch)`, authenticated
origin device, stable mutation ID, per-origin sequence and optional predecessor.
The object key carries its kind as well as UUID, so a song tombstone cannot
alias a playlist or playlist-member identity. Song and playlist-member aliases
are distinct DTO variants and retain their owning playlist where relevant.
Local object revisions and host object versions use separate types.

## Implemented transport trust boundary

The client extracts the provisioned certificate's SPKI and requires the peer's
public key to match. TLS 1.3 CertificateVerify signatures are verified by the
rustls ring provider. A renewed certificate with the same key is accepted;
another key requires renewed trust. Certificate SAN and public CA chains do
not establish this paired identity. System proxies and redirects are disabled.
Pairing, secure-store provisioning and live device revocation remain pending
implementation tasks; this client boundary alone does not implement them.
