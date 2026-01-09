//! 压缩器 - xz 压缩/解压 + SHA-256 校验

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 正确处理 .tar.xz 双扩展名
/// "08.tar.xz" → "08"
/// "08.tar.xz.tmp" → "08"
fn strip_tar_xz_extension(path: &Path) -> PathBuf {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let stem = name
        .strip_suffix(".tar.xz.tmp")
        .or_else(|| name.strip_suffix(".tar.xz"))
        .or_else(|| name.strip_suffix(".xz"))
        .or_else(|| name.strip_suffix(".tar"))
        .unwrap_or(name);
    path.with_file_name(stem)
}

/// 压缩器
pub struct Compressor {
    /// 压缩级别
    level: u8,
    /// 使用多线程
    threads: bool,
    /// 使用 nice 降低优先级
    nice: bool,
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Compressor {
    /// 创建压缩器
    pub fn new() -> Self {
        Self {
            level: 9,
            threads: true,
            nice: true,
        }
    }

    /// 设置压缩级别
    pub fn with_level(mut self, level: u8) -> Self {
        self.level = level.min(9);
        self
    }

    /// 设置是否使用多线程
    pub fn with_threads(mut self, threads: bool) -> Self {
        self.threads = threads;
        self
    }

    /// 设置是否使用 nice
    pub fn with_nice(mut self, nice: bool) -> Self {
        self.nice = nice;
        self
    }

    /// 压缩文件列表到 tar.xz
    pub fn compress_files(&self, files: &[impl AsRef<Path>], output: &Path) -> Result<()> {
        // 创建临时 tar 文件
        // 正确处理 .tar.xz 双扩展名：08.tar.xz → 08.tar
        let tar_path = strip_tar_xz_extension(output).with_extension("tar");

        // 使用 tar 打包
        let mut tar_cmd = Command::new("tar");
        tar_cmd.arg("-cf").arg(&tar_path);

        for file in files {
            tar_cmd.arg(file.as_ref().file_name().unwrap());
            tar_cmd.current_dir(file.as_ref().parent().unwrap());
        }

        // 如果文件来自不同目录，需要特殊处理
        // 这里简化处理：使用绝对路径
        let mut tar_cmd = Command::new("tar");
        tar_cmd.arg("-cf").arg(&tar_path);

        // 添加所有文件（使用 -C 切换目录）
        for file in files {
            let file_path = file.as_ref();
            let dir = file_path.parent().unwrap();
            let name = file_path.file_name().unwrap();
            tar_cmd.arg("-C").arg(dir).arg(name);
        }

        let output_tar = tar_cmd.output().context("执行 tar 失败")?;
        if !output_tar.status.success() {
            anyhow::bail!(
                "tar 打包失败: {}",
                String::from_utf8_lossy(&output_tar.stderr)
            );
        }

        // 使用 xz 压缩
        let mut xz_args = vec![format!("-{}", self.level), "-e".to_string()];
        if self.threads {
            xz_args.push("-T0".to_string());
        }
        xz_args.push(tar_path.to_string_lossy().to_string());

        let xz_output = if self.nice {
            Command::new("nice")
                .args(["-n", "19", "xz"])
                .args(&xz_args)
                .output()
                .context("执行 nice xz 失败")?
        } else {
            Command::new("xz")
                .args(&xz_args)
                .output()
                .context("执行 xz 失败")?
        };

        if !xz_output.status.success() {
            anyhow::bail!(
                "xz 压缩失败: {}",
                String::from_utf8_lossy(&xz_output.stderr)
            );
        }

        // xz 会自动添加 .xz 后缀（27.tar → 27.tar.xz）
        let xz_result = PathBuf::from(format!("{}.xz", tar_path.display()));
        if xz_result != output {
            std::fs::rename(&xz_result, output)?;
        }

        Ok(())
    }

    /// 解压 tar.xz 到目录
    pub fn decompress(&self, archive: &Path, output_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(output_dir)?;

        // 使用 tar 解压
        let output = Command::new("tar")
            .args(["-xJf"])
            .arg(archive)
            .arg("-C")
            .arg(output_dir)
            .output()
            .context("执行 tar 解压失败")?;

        if !output.status.success() {
            anyhow::bail!(
                "tar 解压失败: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(())
    }

    /// 计算文件的 SHA-256
    pub fn sha256_file(&self, path: &Path) -> Result<String> {
        let mut file = std::fs::File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 8192];

        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }

        let result = hasher.finalize();
        Ok(format!("{:x}", result))
    }

    /// 验证归档完整性
    pub fn verify_archive(&self, archive: &Path) -> Result<bool> {
        // 使用 xz -t 测试归档完整性
        let output = Command::new("xz")
            .args(["-t"])
            .arg(archive)
            .output()
            .context("执行 xz 测试失败")?;

        Ok(output.status.success())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_sha256() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");

        let mut file = std::fs::File::create(&file_path).unwrap();
        file.write_all(b"hello world").unwrap();

        let compressor = Compressor::new();
        let hash = compressor.sha256_file(&file_path).unwrap();

        // "hello world" 的 SHA-256
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }
}
