//! Starts Super Bear Adventure inside a trimmed-down Android emulator.
//!
//! Everything lives in %LOCALAPPDATA%\Bearite-PC. The launcher only relies on tools
//! that ship with Windows 10/11 (curl.exe and tar.exe), so no extra libraries are needed.
//!
//! Flow: download Google's emulator + adb + system image (once) -> download the game
//! APK from the user's own release -> create the virtual device -> boot it -> install
//! the game -> start it. The game's own output is never read or logged.

use serde::Serialize;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};

type R<T> = Result<T, String>;

const GAME_PACKAGE: &str = "com.Earthkwak.Platformer";
/// The game APK lives in this repository's own release v1.
const GAME_URL: &str = "https://github.com/Sp1KLe/Bearite-PC/releases/download/v1/SBA.apk";
const GAME_RELEASE_API: &str = "https://api.github.com/repos/Sp1KLe/Bearite-PC/releases/tags/v1";
const REPO_BASE: &str = "https://dl.google.com/android/repository/";
/// Android 13: its x86_64 Google APIs image can translate ARM apps.
const ANDROID_API: u32 = 33;
const AVD_NAME: &str = "Bearite";
/// Unusual port so we do not clash with Android Studio or other emulators.
const EMU_PORT: u16 = 5584;
const OK_MARK: &str = ".bearite_ok";

// ---------------------------------------------------------------- state / events

#[derive(Clone, Serialize)]
struct LaunchState {
    phase: &'static str,
    text: String,
    progress: i32,
}

#[derive(Serialize)]
pub struct Paths {
    data: String,
    mods: String,
}

#[derive(Serialize)]
pub struct EnvInfo {
    /// Android components are installed.
    ready: bool,
    /// The game APK is downloaded.
    game_ready: bool,
    /// Something is still missing and has to be downloaded.
    needs_download: bool,
    /// A download was interrupted and can be continued.
    partial: bool,
}

/// True while a launch is in progress (so the button cannot start two at once).
#[derive(Default)]
pub struct Busy(AtomicBool);

/// phase: "working" | "running" | "idle" | "error"; progress 0..100, or -1 when unknown.
fn state(app: &AppHandle, phase: &'static str, text: impl Into<String>, progress: i32) {
    let _ = app.emit(
        "launch-state",
        LaunchState {
            phase,
            text: text.into(),
            progress,
        },
    );
}

/// A line for the Log page (launcher actions only).
fn log(app: &AppHandle, line: impl Into<String>) {
    let _ = app.emit("launch-log", line.into());
}

// ---------------------------------------------------------------- paths

pub fn data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Bearite-PC")
}

fn sdk_dir() -> PathBuf {
    data_dir().join("sdk")
}
fn adb_path() -> PathBuf {
    sdk_dir().join("platform-tools").join("adb.exe")
}
fn emulator_path() -> PathBuf {
    sdk_dir().join("emulator").join("emulator.exe")
}
fn image_dir() -> PathBuf {
    sdk_dir()
        .join("system-images")
        .join(format!("android-{}", ANDROID_API))
        .join("google_apis")
        .join("x86_64")
}
fn avd_home() -> PathBuf {
    data_dir().join("avd")
}
fn apk_path() -> PathBuf {
    data_dir().join("game").join("game.apk")
}
fn work_dir() -> PathBuf {
    data_dir().join("downloads")
}
fn serial() -> String {
    format!("emulator-{}", EMU_PORT)
}
fn is_done(dir: &Path) -> bool {
    dir.join(OK_MARK).exists()
}
fn components_ready() -> bool {
    is_done(&sdk_dir().join("platform-tools")) && is_done(&sdk_dir().join("emulator")) && is_done(&image_dir())
}

// ---------------------------------------------------------------- commands for the window

#[tauri::command]
pub fn paths() -> Paths {
    let mods = data_dir().join("mods");
    let _ = fs::create_dir_all(&mods);
    Paths {
        data: data_dir().display().to_string(),
        mods: mods.display().to_string(),
    }
}

/// True if some download was interrupted and left a partly downloaded file behind.
fn has_partial() -> bool {
    [work_dir(), data_dir().join("game")].iter().any(|dir| {
        fs::read_dir(dir)
            .map(|entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    e.path().extension().map_or(false, |x| x == "part")
                        && e.metadata().map(|m| m.len() > 0).unwrap_or(false)
                })
            })
            .unwrap_or(false)
    })
}

#[tauri::command]
pub fn env_info() -> EnvInfo {
    let ready = components_ready();
    let game_ready = apk_path().exists();
    EnvInfo {
        ready,
        game_ready,
        needs_download: !ready || !game_ready,
        partial: has_partial(),
    }
}

/// Starts the game. With `download_only` it just downloads what is missing
/// (continuing interrupted downloads) and stops.
#[tauri::command]
pub fn play(
    app: AppHandle,
    busy: tauri::State<'_, Busy>,
    download_only: Option<bool>,
) -> Result<(), String> {
    if busy.0.swap(true, Ordering::SeqCst) {
        return Err("запуск уже идёт".into());
    }
    let download_only = download_only.unwrap_or(false);
    thread::spawn(move || {
        match launch(&app, download_only) {
            Ok(()) => state(&app, "idle", "", 0),
            Err(e) => {
                log(&app, format!("Ошибка: {}", e));
                state(&app, "error", e, 0);
            }
        }
        app.state::<Busy>().0.store(false, Ordering::SeqCst);
    });
    Ok(())
}

/// Deletes everything the launcher downloaded or created (Android, the game, the
/// virtual device). The mods folder is kept.
#[tauri::command]
pub fn reset_all(app: AppHandle, busy: tauri::State<'_, Busy>) -> Result<(), String> {
    if busy.0.swap(true, Ordering::SeqCst) {
        return Err("Сейчас идёт запуск или игра открыта. Закройте её и повторите.".into());
    }
    thread::spawn(move || {
        match do_reset(&app) {
            Ok(()) => state(&app, "reset", "", 0),
            Err(e) => {
                log(&app, format!("Ошибка: {}", e));
                state(&app, "error", e, 0);
            }
        }
        app.state::<Busy>().0.store(false, Ordering::SeqCst);
    });
    Ok(())
}

// ---------------------------------------------------------------- process helpers

fn system_tool(name: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    root.join("System32").join(name)
}

/// A command that never flashes a console window.
fn command(program: &Path) -> Command {
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW
        c.creation_flags(0x0800_0000);
    }
    c
}

/// Points the Android tools at our own folders instead of the user's profile.
fn sdk_env(c: &mut Command) {
    c.env("ANDROID_SDK_ROOT", sdk_dir());
    c.env("ANDROID_HOME", sdk_dir());
    c.env("ANDROID_AVD_HOME", avd_home());
    c.env("ANDROID_USER_HOME", data_dir().join("user"));
}

/// Runs a command to the end and returns (success, stdout + stderr).
fn run(c: &mut Command) -> R<(bool, String)> {
    let out = c
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("не удалось запустить программу: {}", e))?;
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), text))
}

fn adb(args: &[&str]) -> R<(bool, String)> {
    let mut c = command(&adb_path());
    sdk_env(&mut c);
    c.args(args);
    run(&mut c)
}

/// adb command addressed to our emulator.
fn adb_dev(args: &[&str]) -> R<(bool, String)> {
    let sn = serial();
    let mut all: Vec<&str> = vec!["-s", sn.as_str()];
    all.extend_from_slice(args);
    adb(&all)
}

fn device_online() -> bool {
    let sn = serial();
    match adb(&["devices"]) {
        Ok((true, text)) => text
            .lines()
            .any(|l| l.starts_with(sn.as_str()) && l.split_whitespace().nth(1) == Some("device")),
        _ => false,
    }
}

// ---------------------------------------------------------------- downloading

fn report(app: &AppHandle, label: &str, bytes: u64, total: u64) {
    if total > 0 {
        let pct = ((bytes as f64 / total as f64) * 100.0).min(99.0) as i32;
        state(app, "working", format!("Скачивание {}… {}%", label, pct), pct);
    } else {
        state(
            app,
            "working",
            format!("Скачивание {}… {} МБ", label, bytes / 1_048_576),
            -1,
        );
    }
}

/// Downloads `url` to `dest` with curl.exe, reporting progress by watching the file grow.
/// `base` bytes were already downloaded by earlier files; `total` is the overall size.
fn download(app: &AppHandle, label: &str, url: &str, dest: &Path, base: u64, total: u64) -> R<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("не удалось создать папку: {}", e))?;
    }
    let part = dest.with_extension("part");

    let mut last_error = String::new();
    for attempt in 0..4 {
        // Always continue from what is already on disk (-C -), so a failed download
        // can be resumed later and only the remainder is fetched.
        if attempt > 0 {
            thread::sleep(Duration::from_secs(2));
        }
        let mut c = command(&system_tool("curl.exe"));
        c.args(["-L", "--fail", "-sS", "--connect-timeout", "30", "-C", "-"]);
        c.arg("-o").arg(&part).arg(url);
        c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());

        let mut child = c
            .spawn()
            .map_err(|e| format!("curl.exe не запустился: {}", e))?;
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) => {
                    let done = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
                    report(app, label, base + done, total);
                    thread::sleep(Duration::from_millis(500));
                }
                Err(e) => return Err(format!("curl: {}", e)),
            }
        };

        if status.success() {
            fs::rename(&part, dest)
                .map_err(|e| format!("не удалось сохранить {}: {}", dest.display(), e))?;
            return Ok(());
        }
        let mut err_text = String::new();
        if let Some(mut s) = child.stderr.take() {
            let _ = s.read_to_string(&mut err_text);
        }
        // curl error 33: the server refused to continue the file, so start it over.
        if status.code() == Some(33) {
            let _ = fs::remove_file(&part);
        }
        last_error = err_text.trim().to_string();
        log(
            app,
            format!("Скачивание не удалось (попытка {}): {}", attempt + 1, last_error),
        );
    }
    Err(format!("не удалось скачать {}: {}", label, last_error))
}

/// Downloads a small text file (manifests, release info).
fn fetch_text(url: &str, headers: &[&str]) -> R<String> {
    let mut c = command(&system_tool("curl.exe"));
    c.args(["-L", "--fail", "-sS", "-m", "120"]);
    for h in headers {
        c.arg("-H").arg(h);
    }
    c.arg(url);
    let (ok, text) = run(&mut c)?;
    if ok {
        Ok(text)
    } else {
        Err(text.trim().to_string())
    }
}

// ---------------------------------------------------------------- Google's repository manifests

struct Archive {
    url: String,
    size: u64,
}

fn tag_value<'a>(block: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = block.find(&open)? + open.len();
    let end = block[start..].find(&close)? + start;
    Some(block[start..end].trim())
}

/// Finds the download for package `path` in a manifest. `windows_only` picks the
/// Windows x64 build (tools); system images have no host restriction.
fn find_archive(xml: &str, path: &str, windows_only: bool) -> Option<Archive> {
    let open = format!("<remotePackage path=\"{}\"", path);
    // First pass: stable channel only. Second pass: anything that is not obsolete.
    for stable_only in [true, false] {
        let mut rest = xml;
        while let Some(i) = rest.find(&open) {
            let after = &rest[i..];
            let end = after.find("</remotePackage>").unwrap_or(after.len());
            let block = &after[..end];
            rest = &after[end..];

            let head = block.split('>').next().unwrap_or("");
            if head.contains("obsolete=\"true\"") {
                continue;
            }
            if stable_only && !block.contains("channelRef ref=\"channel-0\"") {
                continue;
            }
            for arch in block.split("<archive>").skip(1) {
                let (Some(url), Some(size)) = (tag_value(arch, "url"), tag_value(arch, "size")) else {
                    continue;
                };
                if windows_only {
                    let os = tag_value(arch, "host-os").unwrap_or("");
                    if os != "windows" || url.contains("arm64") || url.contains("aarch64") {
                        continue;
                    }
                }
                return Some(Archive {
                    url: url.to_string(),
                    size: size.parse().unwrap_or(0),
                });
            }
        }
    }
    None
}

/// Tries the newest manifest name first; Google bumps the number now and then.
fn fetch_manifest(subdir: &str, names: &[&str]) -> R<String> {
    for name in names {
        let url = format!("{}{}{}", REPO_BASE, subdir, name);
        if let Ok(text) = fetch_text(&url, &[]) {
            if text.contains("<remotePackage") {
                return Ok(text);
            }
        }
    }
    Err("не удалось получить список компонентов Google (проверьте интернет)".into())
}

// ---------------------------------------------------------------- components

struct Job {
    label: &'static str,
    url: String,
    size: u64,
    dir: PathBuf,
}

/// Unpacks a Google zip (one top-level folder inside) so that its content ends up in `final_dir`.
fn extract(zip: &Path, final_dir: &Path) -> R<()> {
    let tmp = sdk_dir().join("_unpack");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).map_err(|e| format!("не удалось создать папку: {}", e))?;

    let mut c = command(&system_tool("tar.exe"));
    c.arg("-xf").arg(zip).arg("-C").arg(&tmp);
    let (ok, text) = run(&mut c)?;
    if !ok {
        return Err(format!("распаковка не удалась: {}", text.trim()));
    }

    let entries: Vec<PathBuf> = fs::read_dir(&tmp)
        .map_err(|e| format!("не удалось прочитать папку: {}", e))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    let src = if entries.len() == 1 && entries[0].is_dir() {
        entries[0].clone()
    } else {
        tmp.clone()
    };

    let _ = fs::remove_dir_all(final_dir);
    if let Some(parent) = final_dir.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("не удалось создать папку: {}", e))?;
    }
    fs::rename(&src, final_dir).map_err(|e| format!("не удалось переместить файлы: {}", e))?;
    let _ = fs::remove_dir_all(&tmp);
    fs::write(final_dir.join(OK_MARK), b"ok").map_err(|e| format!("не удалось записать отметку: {}", e))
}

fn prepare_components(app: &AppHandle) -> R<()> {
    let sdk = sdk_dir();
    let need_tools = !is_done(&sdk.join("platform-tools"));
    let need_emu = !is_done(&sdk.join("emulator"));
    let need_img = !is_done(&image_dir());
    if !(need_tools || need_emu || need_img) {
        return Ok(());
    }

    log(app, "Скачиваю компоненты Android. Это нужно один раз.");
    state(app, "working", "Получаю список компонентов…", -1);

    let mut jobs: Vec<Job> = Vec::new();
    if need_tools || need_emu {
        let xml = fetch_manifest("", &["repository2-4.xml", "repository2-3.xml", "repository2-2.xml"])?;
        if need_tools {
            let a = find_archive(&xml, "platform-tools", true)
                .ok_or("в списке Google нет platform-tools")?;
            jobs.push(Job {
                label: "adb",
                url: format!("{}{}", REPO_BASE, a.url),
                size: a.size,
                dir: sdk.join("platform-tools"),
            });
        }
        if need_emu {
            let a = find_archive(&xml, "emulator", true).ok_or("в списке Google нет эмулятора")?;
            jobs.push(Job {
                label: "эмулятор",
                url: format!("{}{}", REPO_BASE, a.url),
                size: a.size,
                dir: sdk.join("emulator"),
            });
        }
    }
    if need_img {
        let pkg = format!("system-images;android-{};google_apis;x86_64", ANDROID_API);
        let xml = fetch_manifest(
            "sys-img/google_apis/",
            &["sys-img2-4.xml", "sys-img2-3.xml", "sys-img2-2.xml"],
        )?;
        let a = find_archive(&xml, &pkg, false)
            .ok_or_else(|| format!("в списке Google нет образа {}", pkg))?;
        jobs.push(Job {
            label: "Android",
            url: format!("{}sys-img/google_apis/{}", REPO_BASE, a.url),
            size: a.size,
            dir: image_dir(),
        });
    }

    let total: u64 = jobs.iter().map(|j| j.size).sum();
    let mut done: u64 = 0;
    for job in &jobs {
        // Named after Google's file, so a half-finished download is never mixed up with another one.
        let file_name = job.url.rsplit('/').next().unwrap_or("component.zip");
        let zip = work_dir().join(file_name);
        log(app, format!("Скачиваю {} ({} МБ)", job.label, job.size / 1_048_576));
        download(app, job.label, &job.url, &zip, done, total)?;

        state(app, "working", format!("Распаковка: {}…", job.label), -1);
        log(app, format!("Распаковываю {}", job.label));
        extract(&zip, &job.dir)?;
        let _ = fs::remove_file(&zip);
        done += job.size;
    }
    log(app, "Компоненты Android готовы.");
    Ok(())
}

/// Downloads the game APK (SBA.apk) from release v1 of this repository.
fn download_game(app: &AppHandle) -> R<()> {
    if apk_path().exists() {
        return Ok(());
    }
    state(app, "working", "Подготовка загрузки игры…", -1);
    log(app, "Скачиваю игру (SBA.apk) из релиза v1…");

    // The size only drives the progress bar, so failing to read it is not an error.
    let size = fetch_text(
        GAME_RELEASE_API,
        &["Accept: application/vnd.github+json", "User-Agent: Bearite-PC"],
    )
    .ok()
    .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
    .and_then(|json| {
        json["assets"]
            .as_array()?
            .iter()
            .find(|a| a["name"].as_str() == Some("SBA.apk"))?["size"]
            .as_u64()
    })
    .unwrap_or(0);

    download(app, "игры", GAME_URL, &apk_path(), 0, size)?;
    log(app, "Файл игры скачан.");
    Ok(())
}

/// Stops our emulator and deletes everything the launcher created, except the mods.
fn do_reset(app: &AppHandle) -> R<()> {
    state(app, "working", "Сброс…", -1);
    log(app, "Сбрасываю всё: останавливаю эмулятор и удаляю скачанное…");
    if adb_path().exists() {
        let _ = adb_dev(&["emu", "kill"]);
        thread::sleep(Duration::from_secs(2));
        let _ = adb(&["kill-server"]);
    }

    let targets = [
        sdk_dir(),
        avd_home(),
        data_dir().join("game"),
        work_dir(),
        data_dir().join("user"),
        emulator_log_path(),
    ];
    for target in &targets {
        let mut last = String::new();
        let mut removed = false;
        // Windows may keep files locked for a few seconds after the emulator stops.
        for _ in 0..8 {
            let result = if target.is_dir() {
                fs::remove_dir_all(target)
            } else if target.exists() {
                fs::remove_file(target)
            } else {
                Ok(())
            };
            match result {
                Ok(()) => {
                    removed = true;
                    break;
                }
                Err(e) => {
                    last = e.to_string();
                    thread::sleep(Duration::from_secs(2));
                }
            }
        }
        if !removed {
            return Err(format!(
                "не удалось удалить {}: {}. Закройте окно эмулятора и повторите.",
                target.display(),
                last
            ));
        }
    }
    log(app, "Сброс выполнен: скачанное удалено, моды остались.");
    Ok(())
}

// ---------------------------------------------------------------- the virtual device

/// Diagnostic only: the check reports just the old AEHD driver, while the
/// emulator can also use the Windows Hypervisor Platform, so it never blocks.
fn accel_check(app: &AppHandle) {
    let mut c = command(&emulator_path());
    sdk_env(&mut c);
    c.arg("-accel-check");
    if let Ok((ok, text)) = run(&mut c) {
        let summary: Vec<&str> = text.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
        log(app, format!("Проверка ускорения: {}", summary.join(" | ")));
        if !ok {
            log(app, "Проверка ускорения не пройдена, всё равно пробую запустить эмулятор.");
        }
    }
}

/// Switches on the Windows Hypervisor Platform (Windows asks for permission).
fn enable_hypervisor(app: &AppHandle) -> String {
    log(app, "Включаю компонент Windows «Платформа низкоуровневой оболочки» (нажмите «Да» в запросе Windows)…");
    state(app, "working", "Включаю компонент Windows…", -1);
    let mut ps = command(Path::new("powershell.exe"));
    ps.args([
        "-NoProfile",
        "-Command",
        "try { $p = Start-Process dism.exe -ArgumentList '/online','/enable-feature','/featurename:HypervisorPlatform','/all','/norestart' -Verb RunAs -Wait -PassThru -ErrorAction Stop; Write-Output ('код ' + $p.ExitCode) } catch { Write-Output ('ошибка: ' + $_.Exception.Message) }",
    ]);
    let text = match run(&mut ps) {
        Ok((_, t)) => t.trim().to_string(),
        Err(e) => e,
    };
    log(app, format!("Результат включения компонента: {}", text));
    if text.contains("код 0") || text.contains("код 3010") {
        "Компонент Windows включён. Перезагрузите ПК и нажмите «Играть» снова".to_string()
    } else {
        "Не удалось включить компонент Windows (нужно нажать «Да» в запросе). Включите вручную: Win+R, optionalfeatures, галочка «Платформа низкоуровневой оболочки Windows», затем перезагрузка".to_string()
    }
}

/// Writes the virtual device by hand, so no Java / avdmanager is needed.
fn create_avd() -> R<()> {
    let avd_dir = avd_home().join(format!("{}.avd", AVD_NAME));
    fs::create_dir_all(&avd_dir).map_err(|e| format!("не удалось создать устройство: {}", e))?;
    fs::create_dir_all(data_dir().join("user")).map_err(|e| format!("не удалось создать папку: {}", e))?;

    let ini = format!(
        "avd.ini.encoding=UTF-8\npath={}\ntarget=android-{}\n",
        avd_dir.display(),
        ANDROID_API
    );
    fs::write(avd_home().join(format!("{}.ini", AVD_NAME)), ini)
        .map_err(|e| format!("не удалось записать устройство: {}", e))?;

    let config = format!(
        "AvdId={name}\n\
         avd.ini.displayname={name}\n\
         avd.ini.encoding=UTF-8\n\
         abi.type=x86_64\n\
         hw.cpu.arch=x86_64\n\
         hw.cpu.ncore=4\n\
         hw.ramSize=2048\n\
         hw.gpu.enabled=yes\n\
         hw.gpu.mode=auto\n\
         hw.lcd.width=1280\n\
         hw.lcd.height=720\n\
         hw.lcd.density=240\n\
         hw.keyboard=yes\n\
         hw.mainKeys=no\n\
         hw.camera.back=none\n\
         hw.camera.front=none\n\
         image.sysdir.1=system-images/android-{api}/google_apis/x86_64/\n\
         tag.id=google_apis\n\
         tag.display=Google APIs\n\
         PlayStore.enabled=false\n\
         disk.dataPartition.size=3G\n\
         fastboot.forceColdBoot=yes\n\
         showDeviceFrame=no\n\
         skin.dynamic=yes\n\
         skin.name=1280x720\n",
        name = AVD_NAME,
        api = ANDROID_API
    );
    fs::write(avd_dir.join("config.ini"), config)
        .map_err(|e| format!("не удалось записать настройки устройства: {}", e))
}

fn emulator_log_path() -> PathBuf {
    data_dir().join("emulator.log")
}

/// Last lines of the emulator's own log, for error messages. This is the emulator,
/// not the game: nothing the game does is read.
fn emulator_log_tail() -> String {
    let text = fs::read_to_string(emulator_log_path()).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let from = lines.len().saturating_sub(8);
    lines[from..].join(" | ")
}

fn start_emulator() -> R<Child> {
    let out = fs::File::create(emulator_log_path()).map_err(|e| format!("не удалось создать лог эмулятора: {}", e))?;
    let err = out
        .try_clone()
        .map_err(|e| format!("не удалось создать лог эмулятора: {}", e))?;
    let port = EMU_PORT.to_string();

    let mut c = command(&emulator_path());
    sdk_env(&mut c);
    c.args([
        "-avd",
        AVD_NAME,
        "-port",
        port.as_str(),
        "-no-snapshot",
        "-no-boot-anim",
        "-gpu",
        "auto",
        "-netdelay",
        "none",
        "-netspeed",
        "full",
    ]);
    c.stdin(Stdio::null()).stdout(Stdio::from(out)).stderr(Stdio::from(err));
    c.spawn().map_err(|e| format!("эмулятор не запустился: {}", e))
}

fn wait_boot(app: &AppHandle, mut child: Option<&mut Child>) -> R<()> {
    let start = Instant::now();
    let _ = adb(&["start-server"]);
    loop {
        if let Some(c) = child.as_deref_mut() {
            if let Ok(Some(status)) = c.try_wait() {
                if !status.success() {
                    return Err(format!("эмулятор закрылся ({}). {}", status, emulator_log_tail()));
                }
            }
        }
        if start.elapsed() > Duration::from_secs(600) {
            return Err(format!("Android не загрузился за 10 минут. {}", emulator_log_tail()));
        }
        if device_online() {
            if let Ok((true, text)) = adb_dev(&["shell", "getprop", "sys.boot_completed"]) {
                if text.trim() == "1" {
                    log(app, "Android загрузился.");
                    return Ok(());
                }
            }
        }
        state(app, "working", "Загрузка Android…", -1);
        thread::sleep(Duration::from_secs(2));
    }
}

fn is_installed() -> bool {
    match adb_dev(&["shell", "pm", "path", GAME_PACKAGE]) {
        Ok((true, text)) => text.contains("package:"),
        _ => false,
    }
}

fn install_game(app: &AppHandle) -> R<()> {
    if let Ok((true, abis)) = adb_dev(&["shell", "getprop", "ro.product.cpu.abilist"]) {
        log(app, format!("Android понимает процессоры: {}", abis.trim()));
    }
    if is_installed() {
        log(app, "Игра уже установлена в Android.");
        return Ok(());
    }

    state(app, "working", "Установка игры…", -1);
    log(app, "Устанавливаю игру в Android…");
    let sn = serial();
    let mut c = command(&adb_path());
    sdk_env(&mut c);
    c.args(["-s", sn.as_str(), "install", "-r", "-g"]).arg(apk_path());
    let (ok, text) = run(&mut c)?;
    if ok && text.contains("Success") {
        log(app, "Игра установлена.");
        return Ok(());
    }
    let hint = if text.contains("NO_MATCHING_ABIS") {
        " Игра не подходит этому Android: в ней нет нужного процессорного кода. Пришлите мне это сообщение."
    } else {
        ""
    };
    Err(format!("установка не удалась: {}.{}", text.trim(), hint))
}

fn start_game() -> R<()> {
    let (ok, text) = adb_dev(&[
        "shell",
        "monkey",
        "-p",
        GAME_PACKAGE,
        "-c",
        "android.intent.category.LAUNCHER",
        "1",
    ])?;
    if !ok || text.contains("No activities found") || text.contains("monkey aborted") {
        return Err(format!("игра не запустилась: {}", text.trim()));
    }
    Ok(())
}

/// Waits until the emulator window is closed.
fn wait_until_closed(app: &AppHandle) {
    let mut misses = 0;
    loop {
        thread::sleep(Duration::from_secs(3));
        if device_online() {
            misses = 0;
        } else {
            misses += 1;
            if misses >= 2 {
                break;
            }
        }
    }
    log(app, "Эмулятор закрыт.");
    let _ = adb(&["kill-server"]);
}

// ---------------------------------------------------------------- the whole launch

fn launch(app: &AppHandle, download_only: bool) -> R<()> {
    if download_only {
        log(app, "Докачиваю недостающие файлы.");
    } else {
        log(app, "Запускаю игру. Пока без модов: их подключим на следующем этапе.");
    }
    prepare_components(app)?;
    download_game(app)?;
    if download_only {
        log(app, "Всё скачано. Теперь можно нажимать «Играть с модами».");
        return Ok(());
    }

    let mut child: Option<Child> = None;
    if device_online() {
        log(app, "Эмулятор уже запущен, использую его.");
    } else {
        accel_check(app);
        create_avd()?;
        state(app, "working", "Запуск Android…", -1);
        log(app, "Запускаю эмулятор. Первая загрузка может занять несколько минут.");
        child = Some(start_emulator()?);
    }

    if let Err(e) = wait_boot(app, child.as_mut()) {
        let low = e.to_lowercase();
        if low.contains("accel") || low.contains("whpx") || low.contains("hypervisor") || low.contains("hyper-v") {
            log(app, format!("Эмулятор не стартовал: {}", e));
            return Err(enable_hypervisor(app));
        }
        return Err(e);
    }
    install_game(app)?;

    state(app, "working", "Запуск игры…", -1);
    start_game()?;
    log(app, "Игра запущена.");
    state(app, "running", "Игра запущена", 100);
    wait_until_closed(app);
    Ok(())
}
