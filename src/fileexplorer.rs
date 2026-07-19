use chrono::{DateTime, Local, TimeZone};
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

/// 文件信息结构体
#[derive(Debug)]
pub struct FileInfo {
    pub name: String,
    pub size: u64,
    pub modified: DateTime<Local>,
    pub is_dir: bool,
    pub is_file: bool,
}

impl FileInfo {
    /// 获取人性化的文件大小显示
    pub fn size_human_readable(&self) -> String {
        let size = self.size;
        if size < 1024 {
            format!("{} B", size)
        } else if size < 1024 * 1024 {
            format!("{:.1} KB", size as f64 / 1024.0)
        } else if size < 1024 * 1024 * 1024 {
            format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
        } else {
            format!("{:.1} GB", size as f64 / (1024.0 * 1024.0 * 1024.0))
        }
    }

    pub fn format_modified_time(&self) -> String {
        self.modified.format("%Y-%m-%d %H:%M:%S").to_string()
    }
}

/// 获取文件夹列表
pub fn list_directory(dir_path: &str) -> Result<Vec<FileInfo>, Box<dyn std::error::Error>> {
    let path = Path::new(dir_path);

    if !path.exists() {
        return Err(format!("路径不存在: {}", dir_path).into());
    }

    if !path.is_dir() {
        return Err(format!("不是目录: {}", dir_path).into());
    }

    let mut files = Vec::new();

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;

        // 获取文件名
        let name = entry
            .file_name()
            .into_string()
            .unwrap_or_else(|_| "<无效文件名>".to_string());

        // 获取修改时间并转换为本地时间
        let modified = if let Ok(time) = metadata.modified() {
            // 转换为 DateTime<Local>
            let duration = time.duration_since(UNIX_EPOCH)?;
            let secs = duration.as_secs() as i64;
            let nsecs = duration.subsec_nanos();
            Local
                .timestamp_opt(secs, nsecs)
                .single()
                .unwrap_or(Local::now())
        } else {
            Local::now()
        };

        files.push(FileInfo {
            name,
            size: metadata.len(),
            modified,
            is_dir: metadata.is_dir(),
            is_file: metadata.is_file(),
        });
    }

    // 按文件名排序
    files.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(files)
}
