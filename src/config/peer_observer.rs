//! What the silent peer observer's audio storage needs from the environment:
//! the
//! capture bound, and a directory only the server's user can read.
use std::path::Path;
use std::{fs, io};

/// The longest stretch the silent observer captures, shared by the startup
/// deadline check and the runtime bound so the two cannot drift apart.
pub const MAX_CAPTURE_SECONDS: u64 = 2 * 60 * 60;

/// Creates `dir` private to the server's user, or reports that an existing one
/// is not: audio of candidates must not sit where other users can read it.
pub(crate) fn ensure_private_directory(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(dir)?.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "peer audio directory must be private (0700)",
            ));
        }
    }
    Ok(())
}

/// The static-file server runs as the same user as the private audio writer,
/// so filesystem mode bits cannot protect a directory beneath its web root.
pub fn peer_audio_storage_is_private(
    audio_dir: &std::path::Path,
    web_dir: &std::path::Path,
) -> bool {
    let Ok(audio) = audio_dir.canonicalize() else {
        return false;
    };
    match web_dir.canonicalize() {
        Ok(web) => !audio.starts_with(&web) && !symlink_reaches(&web, &audio),
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

/// A symlink under the web root whose target is the audio directory, or one of
/// its parents, would let the static server read the raw recordings through it.
fn symlink_reaches(dir: &std::path::Path, audio: &std::path::Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            return false;
        };
        if kind.is_symlink() {
            return path
                .canonicalize()
                .is_ok_and(|target| audio.starts_with(&target) || target.starts_with(audio));
        }
        kind.is_dir() && symlink_reaches(&path, audio)
    })
}
