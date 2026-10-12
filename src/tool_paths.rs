use std::{env, ffi::OsString};

pub fn ffmpeg() -> OsString {
    env::var_os("AB_AV1_FFMPEG").unwrap_or_else(|| "ffmpeg".into())
}

pub fn ffprobe() -> OsString {
    env::var_os("AB_AV1_FFPROBE").unwrap_or_else(|| "ffprobe".into())
}

#[test]
fn executable_paths() -> anyhow::Result<()> {
    use anyhow::Context;
    use std::{ffi::OsStr, process::Command};

    const EXPECTED_FFMPEG: &str = "AB_AV1_TEST_FFMPEG";
    const EXPECTED_FFPROBE: &str = "AB_AV1_TEST_FFPROBE";
    const VERIFIED: &str = "ab-av1 tool paths verified";

    if let Some(expected) = env::var_os(EXPECTED_FFMPEG) {
        assert_eq!(ffmpeg(), expected);
        assert_eq!(
            ffprobe(),
            env::var_os(EXPECTED_FFPROBE).context("missing expected ffprobe path")?
        );
        println!("{VERIFIED}");
        return Ok(());
    }

    // Isolate each environment without unsafe process-global mutation.
    let test_binary = env::current_exe()?;
    let check = |(configured_ffmpeg, configured_ffprobe): (Option<&OsStr>, Option<&OsStr>),
                 (expected_ffmpeg, expected_ffprobe): (&OsStr, &OsStr)|
     -> anyhow::Result<()> {
        let mut child = Command::new(&test_binary);
        child
            .args(["--exact", "tool_paths::executable_paths", "--nocapture"])
            .env_remove("AB_AV1_FFMPEG")
            .env_remove("AB_AV1_FFPROBE")
            .env(EXPECTED_FFMPEG, expected_ffmpeg)
            .env(EXPECTED_FFPROBE, expected_ffprobe);
        if let Some(path) = configured_ffmpeg {
            child.env("AB_AV1_FFMPEG", path);
        }
        if let Some(path) = configured_ffprobe {
            child.env("AB_AV1_FFPROBE", path);
        }
        let output = child.output()?;
        anyhow::ensure!(
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains(VERIFIED),
            "ffmpeg={configured_ffmpeg:?}, ffprobe={configured_ffprobe:?}: {}\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    };

    let ffmpeg_name = OsStr::new("ffmpeg");
    let ffprobe_name = OsStr::new("ffprobe");
    let ffmpeg_path = OsStr::new("media tools/ffmpeg tést 🦀");
    let ffprobe_path = OsStr::new("media tools/ffprobe tést 🦀");
    let empty = OsStr::new("");
    for (configured, expected) in [
        ((None, None), (ffmpeg_name, ffprobe_name)),
        ((Some(ffmpeg_path), None), (ffmpeg_path, ffprobe_name)),
        ((None, Some(ffprobe_path)), (ffmpeg_name, ffprobe_path)),
        (
            (Some(ffmpeg_path), Some(ffprobe_path)),
            (ffmpeg_path, ffprobe_path),
        ),
        ((Some(empty), None), (empty, ffprobe_name)),
        ((None, Some(empty)), (ffmpeg_name, empty)),
    ] {
        check(configured, expected)?;
    }

    #[cfg(unix)]
    {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        let ffmpeg = OsString::from_vec(b"ffmpeg \xff".to_vec());
        let ffprobe = OsString::from_vec(b"ffprobe \xfe".to_vec());
        check((Some(&ffmpeg), Some(&ffprobe)), (&ffmpeg, &ffprobe))?;
    }
    #[cfg(windows)]
    {
        use std::{ffi::OsString, os::windows::ffi::OsStringExt};
        let ffmpeg = OsString::from_wide(&[0xd800]);
        let ffprobe = OsString::from_wide(&[0xdfff]);
        check((Some(&ffmpeg), Some(&ffprobe)), (&ffmpeg, &ffprobe))?;
    }
    Ok(())
}
