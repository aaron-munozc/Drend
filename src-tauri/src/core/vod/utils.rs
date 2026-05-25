use std::fs as stdfs;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;

pub async fn run_ffmpeg(
    args: &[&str],
    tmp_dir: &PathBuf,
    log_prefix: &str,
) -> Result<(i32, PathBuf, PathBuf), String> {
    let stderr_path = tmp_dir.join(format!("{}_stderr.log", log_prefix));
    let stdout_path = tmp_dir.join(format!("{}_stdout.log", log_prefix));

    let stderr_file = stdfs::File::create(&stderr_path).map_err(|e| e.to_string())?;
    let stdout_file = stdfs::File::create(&stdout_path).map_err(|e| e.to_string())?;

    #[cfg(target_family = "unix")]
    let mut cmd = {
        let mut c = Command::new("nice");
        c.arg("-n").arg("19").arg("ffmpeg");
        c
    };

    #[cfg(not(target_family = "unix"))]
    let mut cmd = Command::new("ffmpeg");

    cmd.kill_on_drop(true);

    for a in args {
        cmd.arg(a);
    }

    cmd.stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn ffmpeg: {}", e))?;

    let status = child
        .wait()
        .await
        .map_err(|e| format!("Error waiting for ffmpeg: {}", e))?;
    let code = status.code().unwrap_or(-1);

    Ok((code, stderr_path, stdout_path))
}
