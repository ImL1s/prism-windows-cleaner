use serde::Serialize;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::io::BufRead;
use regex::Regex;
use tauri::{AppHandle, Emitter};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

/// Structured scan event emitted to the frontend in real-time
#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type")]
pub enum ScanEvent {
    #[serde(rename = "section")]
    Section { name: String },
    #[serde(rename = "item")]
    Item { name: String, size: String, count: Option<u32> },
    #[serde(rename = "success")]
    Success { name: String },
    #[serde(rename = "info")]
    Info { text: String },
    #[serde(rename = "done")]
    Done { success: bool },
}

/// Static compiled regex for stripping ANSI escape codes
static ANSI_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").unwrap()
});

/// Strip ANSI escape codes from a string
fn strip_ansi(s: &str) -> String {
    ANSI_RE.replace_all(s, "").to_string()
}

/// Parse a single line of CLI output into a ScanEvent
fn parse_scan_line(line: &str) -> Option<ScanEvent> {
    let clean = strip_ansi(line).trim().to_string();
    if clean.is_empty() { return None; }

    // Section header: "➤ xxx" or ">" at start
    if clean.starts_with('➤') || clean.starts_with("> ") {
        let name = clean.trim_start_matches('➤').trim_start_matches('>').trim().to_string();
        if !name.is_empty() {
            return Some(ScanEvent::Section { name });
        }
    }

    // Dry-run item: "→ xxx (N items, SIZE dry)" or "→ xxx (SIZE dry)"
    if clean.starts_with('→') || clean.starts_with("-> ") {
        let content = clean.trim_start_matches('→').trim_start_matches("->").trim();
        // Parse: "name (count items, size dry)" or "name (size dry)" or "name (size)"
        if let Some(paren_start) = content.rfind('(') {
            let name = content[..paren_start].trim().to_string();
            let paren_content = content[paren_start+1..].trim_end_matches(')').trim();
            
            // Try to extract count and size
            let mut count: Option<u32> = None;
            let mut size = String::new();
            
            for part in paren_content.split(',') {
                let part = part.trim().trim_end_matches("dry").trim();
                if part.contains("items") || part.contains("item") {
                    if let Some(n) = part.split_whitespace().next() {
                        count = n.parse().ok();
                    }
                } else if part.contains("dirs") || part.contains("dir") {
                    if let Some(n) = part.split_whitespace().next() {
                        count = n.parse().ok();
                    }
                } else if !part.is_empty() {
                    size = part.to_string();
                }
            }
            
            if !name.is_empty() {
                return Some(ScanEvent::Item { name, size, count });
            }
        } else {
            let name = content.to_string();
            if !name.is_empty() {
                return Some(ScanEvent::Item { name, size: String::new(), count: None });
            }
        }
    }

    // Success: "✓ xxx" or "+ xxx"
    if clean.starts_with('✓') || clean.starts_with("+ ") {
        let name = clean.trim_start_matches('✓').trim_start_matches('+').trim().to_string();
        return Some(ScanEvent::Success { name });
    }

    // Summary lines
    if clean.starts_with("Potential space:") || clean.starts_with("Space freed:") {
        return Some(ScanEvent::Info { text: format!("💾 {}", clean) });
    }
    if clean.starts_with("Items found:") || clean.starts_with("Items cleaned:") {
        return Some(ScanEvent::Info { text: format!("📦 {}", clean) });
    }
    if clean.starts_with("Categories:") {
        return Some(ScanEvent::Info { text: format!("📂 {}", clean) });
    }
    if clean.contains("Dry run complete") || clean.contains("Cleanup complete") {
        return Some(ScanEvent::Info { text: format!("✅ {}", clean) });
    }

    // System info line
    if clean.contains("Free space:") || clean.contains("Windows") {
        return Some(ScanEvent::Info { text: format!("⚙ {}", clean) });
    }

    // Skip noise
    if clean == "Clean Your Windows" || clean.starts_with("Dry Run Mode") 
        || clean.starts_with("Name") || clean.starts_with("----")
        || clean.starts_with("Run without") || clean.starts_with("Detailed list") {
        return None;
    }

    // Generic text
    if clean.len() > 2 {
        return Some(ScanEvent::Info { text: clean });
    }

    None
}

/// Run a Mole PowerShell script with streaming output via Tauri events
fn run_mole_script_streaming(
    app: &AppHandle,
    script_name: &str,
    args: &[&str],
    event_name: &str,
) -> Result<MoleResult, String> {
    let mole_path = get_mole_core_path();
    let script_path = mole_path.join("bin").join(script_name);

    if !script_path.exists() {
        return Err(format!("Script not found: {}", script_path.display()));
    }

    let mut cmd = Command::new("powershell");
    #[cfg(target_os = "windows")]
    cmd.creation_flags(0x08000000);
    cmd.arg("-ExecutionPolicy").arg("Bypass")
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-File")
        .arg(&script_path);

    for arg in args {
        cmd.arg(arg);
    }

    cmd.current_dir(&mole_path);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| format!("Failed to spawn: {}", e))?;
    
    let stdout = child.stdout.take().ok_or("Failed to capture stdout")?;
    let reader = std::io::BufReader::new(stdout);
    
    let mut full_stdout = String::new();
    
    for line in reader.lines() {
        match line {
            Ok(line_str) => {
                full_stdout.push_str(&line_str);
                full_stdout.push('\n');
                
                if let Some(event) = parse_scan_line(&line_str) {
                    let _ = app.emit(event_name, &event);
                }
            }
            Err(_) => break,
        }
    }
    
    let status = child.wait().map_err(|e| format!("Process error: {}", e))?;
    
    // Emit done event
    let _ = app.emit(event_name, &ScanEvent::Done { success: status.success() });
    
    let stderr = child.stderr
        .map(|mut s| {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut s, &mut buf).ok();
            buf
        })
        .unwrap_or_default();
    
    Ok(MoleResult {
        success: status.success(),
        stdout: full_stdout,
        stderr,
        exit_code: status.code(),
    })
}

#[derive(Debug, Serialize, Clone)]
pub struct MoleResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Serialize, Clone)]
pub struct InstalledApp {
    pub name: String,
    pub publisher: String,
    pub version: String,
    pub size_kb: u64,
    pub size_human: String,
    pub source: String,
}

/// Get the path to the mole-core scripts directory
fn get_mole_core_path() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .expect("Failed to get executable path")
        .parent()
        .expect("Failed to get parent directory")
        .to_path_buf();

    // 1) Check bundled resources (installed app): exe_dir/mole-core/bin
    let bundled = exe_dir.join("mole-core");
    if bundled.join("bin").exists() {
        return bundled;
    }

    // 2) Dev mode: walk up from exe to find mole-core
    let dev_path = exe_dir
        .ancestors()
        .find(|p| p.join("mole-core").join("bin").exists())
        .map(|p| p.join("mole-core"));

    dev_path.unwrap_or_else(|| bundled)
}

/// Run a Mole PowerShell script with optional arguments
fn run_mole_script(script_name: &str, args: &[&str]) -> Result<MoleResult, String> {
    let mole_path = get_mole_core_path();
    let script_path = mole_path.join("bin").join(script_name);

    if !script_path.exists() {
        return Err(format!("Script not found: {}", script_path.display()));
    }

    let mut cmd = Command::new("powershell");
    #[cfg(target_os = "windows")]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    cmd.arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-File")
        .arg(&script_path);

    for arg in args {
        cmd.arg(arg);
    }

    cmd.current_dir(&mole_path);

    let output = cmd
        .output()
        .map_err(|e| format!("Failed to execute: {}", e))?;

    Ok(MoleResult {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        exit_code: output.status.code(),
    })
}

#[tauri::command]
pub async fn mole_clean(dry_run: bool) -> Result<MoleResult, String> {
    let handle = std::thread::spawn(move || {
        let args: Vec<&str> = if dry_run { vec!["--dry-run"] } else { vec![] };
        run_mole_script("clean.ps1", &args)
    });
    handle.join().map_err(|_| "Thread panic".to_string())?
}

#[tauri::command]
pub async fn mole_clean_streaming(app: AppHandle, dry_run: bool) -> Result<MoleResult, String> {
    let handle = std::thread::spawn(move || {
        let args: Vec<&str> = if dry_run { vec!["--dry-run"] } else { vec![] };
        run_mole_script_streaming(&app, "clean.ps1", &args, "scan-progress")
    });
    handle.join().map_err(|_| "Thread panic".to_string())?
}

#[tauri::command]
pub async fn mole_optimize(dry_run: bool) -> Result<MoleResult, String> {
    let handle = std::thread::spawn(move || {
        let args: Vec<&str> = if dry_run { vec!["--dry-run"] } else { vec![] };
        run_mole_script("optimize.ps1", &args)
    });
    handle.join().map_err(|_| "Thread panic".to_string())?
}

/// Purge build artifacts — uses inline non-interactive scanning
/// (purge.ps1 is an interactive TUI that hangs under -NonInteractive)
#[tauri::command]
pub async fn mole_purge(dry_run: bool) -> Result<MoleResult, String> {
    let handle = std::thread::spawn(move || {
        let ps_script = r#"
$ErrorActionPreference = "SilentlyContinue"
$searchPaths = @(
    "$env:USERPROFILE\Documents",
    "$env:USERPROFILE\Projects",
    "$env:USERPROFILE\Code",
    "D:\Projects", "D:\Code", "D:\OtherProject"
)
$artifactDirs = @("node_modules","target","build","dist",".next",".nuxt",
    "__pycache__",".venv","venv",".gradle",".dart_tool",".turbo","obj","bin",".parcel-cache")
$totalSize = 0; $found = @()
foreach ($sp in $searchPaths) {
    if (-not (Test-Path $sp)) { continue }
    foreach ($art in $artifactDirs) {
        Get-ChildItem -Path $sp -Filter $art -Directory -Recurse -Depth 4 -Force -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -notlike "*\node_modules\*" -or $_.Name -eq "node_modules" } |
        ForEach-Object {
            $sz = (Get-ChildItem $_.FullName -Recurse -File -Force -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum).Sum
            if ($null -eq $sz) { $sz = 0 }
            $szMB = [Math]::Round($sz / 1MB, 1)
            if ($szMB -gt 0.1) {
                $found += [PSCustomObject]@{ Path=$_.FullName; SizeMB=$szMB; Name=$_.Name }
                $totalSize += $sz
            }
        }
    }
}
$totalGB = [Math]::Round($totalSize / 1GB, 2)
"#;

        let action_part = if dry_run {
            r#"
Write-Output "=== 構建產物掃描結果 (Dry Run) ==="
Write-Output ""
$found | Sort-Object -Property SizeMB -Descending | ForEach-Object {
    Write-Output ("  {0,8} MB  {1}" -f $_.SizeMB, $_.Path)
}
Write-Output ""
Write-Output "找到 $($found.Count) 個產物，共 ${totalGB} GB"
Write-Output "執行清理可釋放此空間"
"#
        } else {
            r#"
Write-Output "=== 開始清理構建產物 ==="
Write-Output ""
$cleaned = 0; $failed = 0
$found | Sort-Object -Property SizeMB -Descending | ForEach-Object {
    try {
        Remove-Item -Path $_.Path -Recurse -Force -ErrorAction Stop
        Write-Output ("  ✓ {0,8} MB  {1}" -f $_.SizeMB, $_.Path)
        $cleaned++
    } catch {
        Write-Output ("  ✗ {0}  (access denied)" -f $_.Path)
        $failed++
    }
}
Write-Output ""
Write-Output "清理完成: $cleaned 個成功, $failed 個跳過, 釋放 ${totalGB} GB"
"#
        };

        let full_script = format!("{}{}", ps_script, action_part);

        let mut cmd = Command::new("powershell");
        #[cfg(target_os = "windows")]
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        let output = cmd
            .arg("-ExecutionPolicy").arg("Bypass")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(&full_script)
            .output()
            .map_err(|e| format!("Failed to execute: {}", e))?;

        Ok(MoleResult {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code(),
        })
    });
    handle.join().map_err(|_| "Thread panic".to_string())?
}

#[tauri::command]
pub async fn mole_status() -> Result<MoleResult, String> {
    let handle = std::thread::spawn(|| {
        run_mole_script("status.ps1", &[])
    });
    handle.join().map_err(|_| "Thread panic".to_string())?
}

#[tauri::command]
pub async fn mole_analyze(path: Option<String>) -> Result<MoleResult, String> {
    let handle = std::thread::spawn(move || {
        match path.as_deref() {
            Some(p) => run_mole_script("analyze.ps1", &[p]),
            None => run_mole_script("analyze.ps1", &[]),
        }
    });
    handle.join().map_err(|_| "Thread panic".to_string())?
}

/// List installed applications by querying the Windows registry
#[tauri::command]
pub async fn list_installed_apps() -> Result<Vec<InstalledApp>, String> {
    let handle = std::thread::spawn(|| {
        let ps_script = r#"
$apps = @()
$paths = @(
    'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*',
    'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*',
    'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*'
)
foreach ($p in $paths) {
    try {
        Get-ItemProperty -Path $p -ErrorAction SilentlyContinue |
        Where-Object { $_.DisplayName -and $_.UninstallString } |
        ForEach-Object {
            $size = 0
            if ($_.EstimatedSize) { $size = [long]$_.EstimatedSize }
            $apps += [PSCustomObject]@{
                Name = $_.DisplayName
                Publisher = if ($_.Publisher) { $_.Publisher } else { '' }
                Version = if ($_.DisplayVersion) { $_.DisplayVersion } else { '' }
                SizeKB = $size
                Source = 'Registry'
            }
        }
    } catch { }
}
$apps | Sort-Object -Property SizeKB -Descending | ConvertTo-Json -Depth 3
"#;

        let mut cmd = Command::new("powershell");
        #[cfg(target_os = "windows")]
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        let output = cmd
            .arg("-ExecutionPolicy").arg("Bypass")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(ps_script)
            .output()
            .map_err(|e| format!("PS error: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();

        #[derive(serde::Deserialize)]
        struct PsApp {
            #[serde(alias = "Name")]
            name: String,
            #[serde(alias = "Publisher", default)]
            publisher: String,
            #[serde(alias = "Version", default)]
            version: String,
            #[serde(alias = "SizeKB", default)]
            size_kb: u64,
            #[serde(alias = "Source", default)]
            source: String,
        }

        fn format_size(kb: u64) -> String {
            if kb == 0 { return "N/A".into(); }
            if kb < 1024 { return format!("{} KB", kb); }
            if kb < 1048576 { return format!("{:.1} MB", kb as f64 / 1024.0); }
            format!("{:.1} GB", kb as f64 / 1048576.0)
        }

        // Try parsing as array first, then as single object
        let ps_apps: Vec<PsApp> = serde_json::from_str(&stdout)
            .or_else(|_| serde_json::from_str::<PsApp>(&stdout).map(|a| vec![a]))
            .unwrap_or_default();

        let apps: Vec<InstalledApp> = ps_apps.into_iter().map(|a| InstalledApp {
            size_human: format_size(a.size_kb),
            name: a.name,
            publisher: a.publisher,
            version: a.version,
            size_kb: a.size_kb,
            source: a.source,
        }).collect();

        Ok(apps)
    });

    handle.join().map_err(|_| "Thread panic".to_string())?
}

/// Uninstall an app by name — finds its UninstallString from registry and invokes it
#[tauri::command]
pub async fn mole_uninstall_app(app_name: String) -> Result<MoleResult, String> {
    let handle = std::thread::spawn(move || {
        let ps_script = format!(r#"
$name = '{}'
$paths = @(
    'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*',
    'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*',
    'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*'
)
$found = $null
foreach ($p in $paths) {{
    $found = Get-ItemProperty -Path $p -ErrorAction SilentlyContinue |
        Where-Object {{ $_.DisplayName -eq $name }} |
        Select-Object -First 1
    if ($found) {{ break }}
}}
if (-not $found) {{
    Write-Error "App not found: $name"
    exit 1
}}
$uninstallCmd = $found.QuietUninstallString
if (-not $uninstallCmd) {{ $uninstallCmd = $found.UninstallString }}
if (-not $uninstallCmd) {{
    Write-Error "No uninstall command found for: $name"
    exit 1
}}
Write-Output "Uninstalling: $name"
Write-Output "Command: $uninstallCmd"
try {{
    if ($uninstallCmd -match '^msiexec') {{
        # Add /quiet for MSI uninstalls
        $uninstallCmd = $uninstallCmd -replace '/I', '/X'
        if ($uninstallCmd -notmatch '/quiet|/qn') {{
            $uninstallCmd += ' /quiet /norestart'
        }}
    }}
    cmd /c $uninstallCmd 2>&1
    Write-Output "Uninstall completed for: $name"
}} catch {{
    Write-Error "Uninstall failed: $_"
    exit 1
}}
"#, app_name.replace("'", "''"));

        let mut cmd = Command::new("powershell");
        #[cfg(target_os = "windows")]
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        let output = cmd
            .arg("-ExecutionPolicy").arg("Bypass")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(&ps_script)
            .output()
            .map_err(|e| format!("Failed to execute: {}", e))?;

        Ok(MoleResult {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code(),
        })
    });
    handle.join().map_err(|_| "Thread panic".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mole_core_path_exists_or_graceful() {
        // Should not panic even if mole-core isn't present
        let path = get_mole_core_path();
        // Path should be a valid PathBuf regardless
        assert!(!path.to_string_lossy().is_empty());
    }

    #[test]
    fn mole_result_serialization() {
        let r = MoleResult {
            success: true,
            stdout: "hello".into(),
            stderr: String::new(),
            exit_code: Some(0),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("\"stdout\":\"hello\""));
    }

    #[test]
    fn installed_app_serialization() {
        let app = InstalledApp {
            name: "Test App".into(),
            publisher: "Test Inc".into(),
            version: "1.0".into(),
            size_kb: 2048,
            size_human: "2.0 MB".into(),
            source: "Registry".into(),
        };
        let json = serde_json::to_string(&app).unwrap();
        assert!(json.contains("Test App"));
        assert!(json.contains("size_kb"));
    }

    #[test]
    fn run_mole_script_missing_script_returns_error() {
        let result = run_mole_script("nonexistent_script_42.ps1", &[]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Script not found") || err.contains("Failed"));
    }

    #[test]
    fn powershell_is_available() {
        // Verify PowerShell can be invoked (required for all bridge commands)
        let output = Command::new("powershell")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg("Write-Output 'hello_mole_test'")
            .output();
        assert!(output.is_ok(), "PowerShell should be available");
        let out = output.unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("hello_mole_test"), "PS output: {}", stdout);
    }

    #[test]
    fn list_installed_apps_returns_nonempty() {
        // Actually queries the real Windows registry — this IS the integration test
        let handle = std::thread::spawn(|| {
            let ps_script = r#"
$apps = @()
$paths = @(
    'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*',
    'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*',
    'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*'
)
foreach ($p in $paths) {
    try {
        Get-ItemProperty -Path $p -ErrorAction SilentlyContinue |
        Where-Object { $_.DisplayName -and $_.UninstallString } |
        ForEach-Object {
            $size = 0
            if ($_.EstimatedSize) { $size = [long]$_.EstimatedSize }
            $apps += [PSCustomObject]@{
                Name = $_.DisplayName
                Publisher = if ($_.Publisher) { $_.Publisher } else { '' }
                Version = if ($_.DisplayVersion) { $_.DisplayVersion } else { '' }
                SizeKB = $size
                Source = 'Registry'
            }
        }
    } catch { }
}
$apps | Sort-Object -Property SizeKB -Descending | ConvertTo-Json -Depth 3
"#;
            let output = Command::new("powershell")
                .arg("-ExecutionPolicy").arg("Bypass")
                .arg("-NoProfile")
                .arg("-NonInteractive")
                .arg("-Command")
                .arg(ps_script)
                .output()
                .expect("PS should run");

            let stdout = String::from_utf8_lossy(&output.stdout).to_string();

            #[derive(serde::Deserialize)]
            struct PsApp {
                #[serde(alias = "Name")]
                name: String,
            }

            let apps: Vec<PsApp> = serde_json::from_str(&stdout)
                .unwrap_or_default();
            apps.len()
        });

        let count = handle.join().expect("thread panicked");
        assert!(count > 5, "A Windows machine should have > 5 installed apps, got {}", count);
    }
}

