//! The language the interface is drawn in.
//!
//! One language is chosen at startup — the system's unless the setting says
//! otherwise — and every screen reads its words from here. The English text
//! is the key: a call site wraps its literal in [`tr`], and the table below
//! gives the Chinese, falling back to the literal when it has no entry. That
//! keeps a screen readable in place and makes a missing translation a missing
//! line rather than a panic.
//!
//! `format!` cannot help with a translated string, because a format string
//! must be a literal and the translation is chosen at run time. Strings with
//! parts interpolated are written with `{}` and filled by [`text!`] instead,
//! which is all the interface ever needed.
//!
//! The choice is saved beside the Power Efficiency preset, in the same
//! per-desktop configuration directory, and takes effect on the frame after it
//! is made. The macOS menu bar is built once, so it follows the language the
//! app started in until the next launch.

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};

/// A language the interface is actually drawn in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Locale {
    #[default]
    En,
    Zh,
}

impl Locale {
    const fn code(self) -> u8 {
        match self {
            Self::En => 0,
            Self::Zh => 1,
        }
    }

    const fn from_code(code: u8) -> Self {
        if code == 1 { Self::Zh } else { Self::En }
    }

    /// This machine's language, from the environment.
    pub fn system() -> Self {
        #[cfg(windows)]
        if let Some(name) = windows_locale() {
            return Self::from_tag(&name);
        }
        // The POSIX order: LC_ALL outranks LC_MESSAGES, which outranks LANG.
        for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(value) = std::env::var(key)
                && !value.is_empty()
            {
                return Self::from_tag(&value);
            }
        }
        Self::En
    }

    /// The language a locale tag like `zh_CN.UTF-8` or `zh-Hans-CN` names.
    fn from_tag(tag: &str) -> Self {
        let lower = tag.to_ascii_lowercase();
        if lower.starts_with("zh") {
            Self::Zh
        } else {
            Self::En
        }
    }
}

/// The saved preference: a language, or whatever the system is set to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    System,
    English,
    Chinese,
}

impl Language {
    /// The three choices, in the order the menu lists them.
    pub const ALL: [Self; 3] = [Self::System, Self::English, Self::Chinese];

    pub const fn key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::English => "english",
            Self::Chinese => "chinese",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|language| language.key() == value)
    }

    /// The concrete language to draw in.
    pub fn resolve(self) -> Locale {
        match self {
            Self::System => Locale::system(),
            Self::English => Locale::En,
            Self::Chinese => Locale::Zh,
        }
    }

    /// The name shown in the button and the menu. A language is named in
    /// itself, so the choice is readable whoever is looking at it.
    pub fn name(self) -> &'static str {
        match self {
            Self::System => tr("System"),
            Self::English => "English",
            Self::Chinese => "中文",
        }
    }
}

/// The language in force, as a byte so it can live in a static without a lock.
static CURRENT: AtomicU8 = AtomicU8::new(Locale::En.code());

/// Choose the language every screen draws in.
pub fn set(locale: Locale) {
    CURRENT.store(locale.code(), Ordering::Relaxed);
}

pub fn current() -> Locale {
    Locale::from_code(CURRENT.load(Ordering::Relaxed))
}

/// `text` in the language in force.
pub fn tr(text: &'static str) -> &'static str {
    translated(current(), text)
}

/// `text` in `locale`. Pure, so a test can ask for either language without
/// touching the process-wide choice.
pub fn translated(locale: Locale, text: &'static str) -> &'static str {
    match locale {
        Locale::En => text,
        Locale::Zh => chinese(text).unwrap_or(text),
    }
}

/// The current language for a string known only at run time, like a removal
/// guard's reason, when it has an entry; the text unchanged otherwise.
pub fn tr_str(text: &str) -> &str {
    if current() == Locale::Zh
        && let Some(found) = chinese(text)
    {
        return found;
    }
    text
}

/// Fill each `{}` in `template` with the argument in that position.
///
/// Written by hand rather than through `format!` so a translated template can
/// be used: the scan is one pass, so a value containing `{}` is never read as
/// a placeholder of its own.
pub fn substitute(template: &str, args: &[&dyn std::fmt::Display]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    let mut args = args.iter();
    while let Some(at) = rest.find("{}") {
        out.push_str(&rest[..at]);
        if let Some(arg) = args.next() {
            out.push_str(&arg.to_string());
        }
        rest = &rest[at + 2..];
    }
    out.push_str(rest);
    out
}

/// A translated template with its `{}` placeholders filled, in order.
///
/// `text!("{} marked · {} to free", count, human_bytes(plan.bytes()))`
macro_rules! text {
    ($template:literal $(, $arg:expr)* $(,)?) => {{
        let args: &[&dyn ::std::fmt::Display] = &[$(&$arg),*];
        $crate::i18n::substitute($crate::i18n::tr($template), args)
    }};
}
pub(crate) use text;

/// Where the language choice is saved. `None` when the desktop names no
/// configuration directory, in which case the choice lasts the session.
pub fn settings_path() -> Option<PathBuf> {
    crate::power::config_dir().map(|dir| dir.join("language"))
}

/// The saved choice, or the system's when nothing usable is saved.
pub fn saved() -> Language {
    settings_path()
        .map_or(Language::System, |path| load(&path).unwrap_or_default())
}

pub fn load(path: &Path) -> io::Result<Language> {
    match std::fs::read_to_string(path) {
        Ok(value) => Language::parse(value.trim()).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "unknown language")
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(Language::default())
        }
        Err(error) => Err(error),
    }
}

pub fn save(path: &Path, language: Language) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "no settings directory")
    })?;
    std::fs::create_dir_all(parent)?;
    // Atomic replacement, as the power preset is saved: a failed write leaves
    // the previous choice intact and no reader sees a half-written file.
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    writeln!(file, "{}", language.key())?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

/// The system's locale on Windows, where `LANG` is usually unset.
#[cfg(windows)]
fn windows_locale() -> Option<String> {
    #![allow(unsafe_code, reason = "one Win32 call into a stack buffer")]

    use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;

    // LOCALE_NAME_MAX_LENGTH, which the API caps at.
    let mut buffer = [0u16; 85];
    // SAFETY: the pointer and length describe `buffer`, which outlives the
    // call; the API writes at most `85` UTF-16 units and returns the count,
    // terminator included, or zero on failure.
    let written = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), 85) };
    let length = usize::try_from(written).ok()?.checked_sub(1)?;
    Some(String::from_utf16_lossy(
        &buffer[..length.min(buffer.len())],
    ))
}

/// The Chinese for an English string, or `None` when it has no entry and is
/// shown as it is. A `{}` stands for a part filled in later.
// Several phrases share one Chinese rendering; listing them per English line
// keeps the table in the order a reader meets the screens.
#[allow(
    clippy::match_same_arms,
    reason = "several English strings translate to one Chinese phrase; \
              separate arms keep the table in English reading order"
)]
fn chinese(text: &str) -> Option<&'static str> {
    let translated = match text {
        // ── the trail, the settings row and the legend ──────────────────
        "Scan from here · what is below is reused" => {
            "从这里扫描 · 下方内容会被复用"
        }
        "Siblings in {}" => "{} 中的同级项",
        "+{} smaller" => "另有 {} 个更小的",
        "Size" | "size" => "大小",
        "Files" | "files" => "文件",
        "Age" => "时间",
        "Hidden files" => "隐藏文件",
        "Apparent size" => "表观大小",
        "Depth {}" => "层级 {}",
        "Levels drawn at once · [ and ]" => "一次绘制的层数 · [ 和 ]",
        "Custom" => "自定义",
        "Reclaimable" => "可回收",
        "This week" => "本周",
        "This month" => "本月",
        "Six months" => "半年",
        "This year" => "今年",
        "Older" => "更早",
        "Code" => "代码",
        "Agent scratch" => "智能体暂存",
        "Toolchains" => "工具链",
        "Synced" => "同步",
        "Git" => "Git",
        "Media" => "媒体",
        "Documents" => "文档",
        "Cache" => "缓存",
        "Other" => "其他",
        // ── power efficiency ────────────────────────────────────────────
        "Miser" => "省电",
        "Balanced" => "均衡",
        "Aggressive" => "激进",
        "Drain My Battery" => "榨干电池",
        "worker" | "workers" => "线程",
        "Scan workers" => "扫描线程",
        "{} CPUs" => "{} 个 CPU",
        "Same as {} with {} CPUs" => "与 {} 相同（{} 个 CPU）",
        "Power Efficiency: {}" => "能效：{}",
        "Power Efficiency: {} · {} of {} CPUs" => {
            "能效：{} · {} 个 CPU 中的 {}"
        }
        "Used from the next scan, and remembered." => {
            "从下次扫描起生效，并会记住。"
        }
        "Up to about 8 workers a scan finishes sooner: a whole drive took a \
         third less time with 8 than with Balanced. Past that it is no \
         faster, but uses over twice the CPU and runs the fans at full speed." => {
            "大约 8 个线程以内，扫描会更快：整盘扫描用 8 个线程比“均衡”少\
             三分之一的时间。超过后速度不再提升，但 CPU 占用翻倍以上，风扇\
             会全速运转。"
        }
        "More workers finish a scan sooner only up to a point. Past it they \
         use more CPU for no gain, and can run the fans at full speed." => {
            "更多线程只能让扫描快到一个点。超过后只会占用更多 CPU 而无收益，\
             并可能让风扇全速运转。"
        }
        // ── the scan totals and the status line ─────────────────────────
        "· {} files · {} dirs" => "· {} 个文件 · {} 个目录",
        "· {} unreadable" => "· {} 项无法读取",
        "mark" => "标记",
        "open" => "打开",
        "up" => "上一级",
        "review" => "复核",
        "move" => "移动",
        "filter" => "筛选",
        "depth" => "层级",
        "mode" => "模式",
        "reset" => "重置",
        "volumes" => "卷",
        "rescan" => "重新扫描",
        "all keys" => "全部按键",
        "scanning · {} entries · {}" => "扫描中 · {} 项 · {}",
        "scan cancelled · {} entries · r scans again" => {
            "扫描已取消 · {} 项 · 按 r 重新扫描"
        }
        "scan {} entries" => "扫描 {} 项",
        " · {} s" => " · {} 秒",
        // ── the side panel: selection, worth a look, marked ─────────────
        "Selection" => "选择",
        "Point at a tile or select one with the arrows" => {
            "指向某个色块，或用方向键选择"
        }
        "Kind" => "类型",
        "not readable" => "无法读取",
        "asking…" => "查询中…",
        "Of scan" => "占扫描",
        "Last write" => "最后写入",
        "Marked" => "已标记",
        "Marked · {}" => "已标记 · {}",
        "Goes with {}" => "将随 {} 一起删除",
        "Partly unreadable" => "部分无法读取",
        "Unmark {}" => "取消标记 {}",
        "Unmark" => "取消标记",
        "Open" => "打开",
        "Mark for removal" => "标记为待删除",
        "Worth a look" => "值得一看",
        "Nothing obviously disposable" => "没有明显可删的内容",
        "Waiting for the scan" => "等待扫描",
        "{} worktree{} · oldest {} d" => "{} 个 worktree{} · 最早 {} 天",
        "{} > {} days" => "{} 超过 {} 天",
        "{} experiment{} untouched" => "{} 个实验{}未改动",
        "regenerable" => "可再生",
        "sync history" => "同步历史",
        "package store" => "软件包仓库",
        "build output" => "构建产物",
        "reinstallable" => "可重装",
        "sandbox layers" => "沙箱层",
        "snapshots" => "快照",
        "temporary" => "临时文件",
        "Space marks the tile you point at" => "按空格标记指向的色块",
        "+{} more on the review screen" => "另有 {} 项在复核页",
        "{} nested · {} kept back" => "{} 项嵌套 · {} 项保留",
        // ── the disk meter and the volumes dialog ───────────────────────
        "Disk" => "磁盘",
        "Volumes" => "卷",
        "Free space is not available here" => "此处无法获取可用空间",
        "{} free" => "可用 {}",
        "→ {} free" => "→ 可用 {}",
        "not counted here: {}" => "此处未计入：{}",
        "{} used" => "已用 {}",
        "{} total" => "总计 {}",
        "{} on other volumes" => "{} 在其他卷上",
        "{} could not be placed" => "{} 无法归属",
        "Review {} marked · frees {}…" => "复核 {} 项 · 可释放 {}…",
        "Looking for volumes…" => "正在查找卷…",
        "No other volume could be read." => "无法读取其他卷。",
        "unknown free" => "可用空间未知",
        "Scan a volume" => "扫描某个卷",
        "Up and down moves, Enter scans it, Escape stays here." => {
            "上下键移动，回车扫描，Esc 留在这里。"
        }
        // ── the one dialog ──────────────────────────────────────────────
        "Delete “{}” permanently?" => "永久删除“{}”？",
        "Delete {} items permanently?" => "永久删除 {} 个项目？",
        "This frees {}. Deleted files can’t be recovered; move them to the \
         trash if you might need them again." => {
            "将释放 {}。删除的文件无法恢复；若以后可能再用到，请改为移到回收站。"
        }
        "Cancel" => "取消",
        "Delete" => "删除",
        // ── scanning, the find field, and the key bar ───────────────────
        "Stopped reading {}" => "已停止读取 {}",
        "Reading {}" => "正在读取 {}",
        "directories" => "目录",
        "measured" => "已计量",
        "unreadable" => "无法读取",
        "Nothing is shown from a scan that did not finish." => {
            "未完成的扫描不会显示任何内容。"
        }
        "Marking, zooming and the free-space meter all work as soon as it \
         lands." => "扫描完成后即可标记、缩放并使用可用空间计量。",
        "Scan again" => "再次扫描",
        "searching…" => "搜索中…",
        "type to filter" => "输入以筛选",
        "no matches" => "无匹配",
        "esc clears" => "Esc 清除",
        "{} match{} · {}" => "{} 项匹配{} · {}",
        "enter shows only these" => "回车只显示这些",
        "Filter by name" => "按名称筛选",
        // ── the review screen ───────────────────────────────────────────
        "Nothing is marked. Go back and mark what should go." => {
            "尚未标记任何内容。返回并标记要删除的内容。"
        }
        "{} more are marked and will be removed too. Unmark them in the \
         treemap." => "另有 {} 项已标记，也会被删除。请在树图中取消标记。",
        "Review" => "复核",
        "{} marked · {} to free" => "已标记 {} 项 · 可释放 {}",
        "Inside a marked directory" => "位于已标记的目录内",
        "Move to trash" => "移到回收站",
        "Delete permanently" => "永久删除",
        "Recoverable from the trash until it is emptied. Uses {}." => {
            "在清空回收站之前可以恢复。使用 {}。"
        }
        "Deleted at once, like rm -rf. Nothing is recoverable." => {
            "立即删除，相当于 rm -rf。无法恢复。"
        }
        "What happens" => "将执行的操作",
        "Totals" => "合计",
        "Acted on" => "实际处理",
        "Nested, go with a parent" => "嵌套，随父项一起",
        "Kept back" => "已保留",
        "Space freed" => "释放空间",
        "Not counted here" => "此处未计入",
        "Volume" => "卷",
        "{}: {}" => "{}：{}",
        "the Trash" => "废纸篓",
        "XDG trash" => "XDG 回收站",
        "the Recycle Bin" => "回收站",
        "no trash tool found" => "未找到回收站工具",
        "Save list…" => "保存列表…",
        "Copy as prompt" => "复制为提示词",
        "Back" => "返回",
        "Forward" => "前进",
        "Move {} {} to trash" => "将 {} {}移到回收站",
        "Delete {} {}…" => "删除 {} {}…",
        "move to trash" => "移到回收站",
        "delete…" => "删除…",
        "trash" => "回收站",
        "permanent" => "永久删除",
        "unmark all" => "全部取消标记",
        "save list" => "保存列表",
        "copy as prompt" => "复制为提示词",
        "back" => "返回",
        "{} available" => "可用 {}",
        "alt ← · side button" => "alt ← · 侧键",
        "alt → · side button" => "alt → · 侧键",
        // ── running and done ────────────────────────────────────────────
        "Removing" => "正在删除",
        "{} of {} done" => "已完成 {} / {}",
        "{} removed · {} to free" => "已删除 {} · 释放 {}",
        "stop after the current item" => "当前项完成后停止",
        "removed" => "已删除",
        "{} items" => "{} 项",
        "bytes claimed" => "声称释放字节",
        "failed" => "失败",
        "volume freed" => "卷已释放",
        "The treemap is being re-scanned so the numbers on screen match the \
         disk again." => "正在重新扫描树图，使屏幕上的数字与磁盘一致。",
        "re-scanning" => "重新扫描中",
        "{} files" => "{} 个文件",
        "what could not be removed" => "无法删除的内容",
        "Done" => "完成",
        "removal finished" => "删除已完成",
        "continue" => "继续",
        // ── tooltips and the keyboard overlay ───────────────────────────
        "space mark · enter open" => "空格 标记 · 回车 打开",
        "space mark" => "空格 标记",
        "{} files · {} dirs · {} direct" => {
            "{} 个文件 · {} 个目录 · {} 直接占用"
        }
        "Hidden" => "隐藏",
        "Marked for removal" => "已标记待删除",
        "Inside marked {}" => "位于已标记的 {} 内",
        "Keyboard and mouse" => "键盘与鼠标",
        "space / x" => "空格 / x",
        "Mark or unmark the tile you point at" => "标记或取消标记指向的色块",
        "⌘-click" => "⌘-点击",
        "ctrl-click" => "ctrl-点击",
        "Mark without moving the selection" => "标记但不移动选择",
        "enter" => "回车",
        "Open that directory, at any depth" => "打开该目录（任意深度）",
        "⌫ / esc" => "⌫ / Esc",
        "Go up one directory" => "向上一级目录",
        "Back or forward, as do the mouse's side buttons" => {
            "后退或前进，鼠标侧键亦可"
        }
        "Move between tiles at this level" => "在本层级内移动",
        "Next largest sibling" => "下一个最大的同级项",
        "scroll" => "滚动",
        "Zoom toward a directory, then go into it" => "向某目录缩放，随后进入",
        "shift-scroll" => "Shift+滚动",
        "Pan the magnified view" => "平移放大后的视图",
        "Draw fewer or more levels at once" => "减少或增加一次绘制的层级",
        "Magnify, shrink, or reset the view" => "放大、缩小或重置视图",
        "Interface zoom" => "界面缩放",
        "Filter by name: only matches keep their colour; enter shows only \
         them" => "按名称筛选：仅匹配项保留颜色；回车只显示它们",
        "Review the marked list" => "复核已标记列表",
        "Size, files or age: what areas and colours say" => {
            "大小、文件数或时间：面积与颜色所表示的含义"
        }
        "Scan again from the same root" => "从同一根目录重新扫描",
        "Stop a scan in progress" => "停止正在进行的扫描",
        "Scan another volume" => "扫描另一个卷",
        "Choose another directory to scan" => "选择其他要扫描的目录",
        "The whole disk; click any directory above to widen" => {
            "整个磁盘；点击上方的任意目录可扩大范围"
        }
        "Disk usage or apparent size" => "磁盘占用或表观大小",
        "Include or skip hidden entries" => "包含或跳过隐藏项",
        "Show or hide the selection line" => "显示或隐藏选择栏",
        "Show it in Finder" => "在访达中显示",
        "Show it in File Explorer" => "在文件资源管理器中显示",
        "Show it in the file manager" => "在文件管理器中显示",
        "Quit" => "退出",
        "Review screen" => "复核页",
        "m trash · p permanent · ! unmark all · s save list · a copy as \
         prompt" => {
            "m 回收站 · p 永久删除 · ! 全部取消标记 · s 保存列表 · a \
               复制为提示词"
        }
        "enter commits · esc goes back" => "回车提交 · Esc 返回",
        "A permanent deletion always asks first" => "永久删除总会先确认",
        "? or esc closes · {} · {}" => "? 或 Esc 关闭 · {} · {}",
        // ── notices ─────────────────────────────────────────────────────
        "{} from the next scan; r rescans now" => {
            "{}，从下次扫描起生效；按 r 立即重新扫描"
        }
        "{} from the next scan, but it could not be saved: {}" => {
            "{}，从下次扫描起生效，但无法保存：{}"
        }
        "{} from the next scan; no settings directory to save it in" => {
            "{}，从下次扫描起生效；没有可保存的设置目录"
        }
        "Language: {}. Saved." => "语言：{}。已保存。",
        "Language: {}. It could not be saved: {}" => "语言：{}。无法保存：{}",
        "Language: {}. No settings directory to save it in" => {
            "语言：{}。没有可保存的设置目录"
        }
        "System" => "跟随系统",
        "Interface language" => "界面语言",
        "Interface language: {}" => "界面语言：{}",
        "progress" => "进度",
        "esc" => "Esc",
        "the scanned root cannot be removed; open a directory first" => {
            "不能删除扫描根目录；请先打开某个目录"
        }
        "{} goes with the marked {}; unmark that to keep it" => {
            "{} 将随已标记的 {} 一起删除；要保留请取消该标记"
        }
        "{} now covers {} mark{} inside it" => {
            "{} 现在涵盖其内部的 {} 个标记{}"
        }
        "+{} more" => "另有 {} 项",
        "nothing is marked" => "尚未标记任何内容",
        "not while restarting as administrator" => {
            "正在以管理员身份重启，暂不可用"
        }
        "nothing here matches {}" => "此处没有任何内容匹配 {}",
        "nothing to save" => "没有可保存的内容",
        "saved {} {} to {}" => "已保存 {} 个{}到 {}",
        "path" | "paths" => "路径",
        "could not save {}: {}" => "无法保存 {}：{}",
        "nothing to copy" => "没有可复制的内容",
        "copied a prompt for your agent: {} {}, {}" => {
            "已复制给智能体的提示词：{} 个{}，{}"
        }
        "{} is no longer on disk" => "{} 已不在磁盘上",
        "the scanned root cannot be removed" => "不能删除扫描根目录",
        "the home directory cannot be removed" => "不能删除主目录",
        "it contains the home directory" => "它包含主目录",
        "a user profile cannot be removed" => "不能删除用户配置文件",
        "outside the scanned root" => "在扫描根目录之外",
        "a mount point: removing it would cross onto another filesystem" => {
            "是挂载点：删除它会跨入另一个文件系统"
        }
        "stopping after the current item" => "当前项完成后停止",
        "mark something first: space marks the selected tile" => {
            "请先标记：按空格标记选中的色块"
        }
        "still not an administrator: {}" => "仍不是管理员：{}",
        "Scan" => "扫描",
        // ── the privacy and administrator hints ─────────────────────────
        "macOS kept {} {} unreadable. Give disktree Full Disk Access, or \
         your terminal if you started it there, then reopen it." => {
            "macOS 使 {} {}无法读取。请为 disktree 授予“完全磁盘访问权限”\
             （若从终端启动则授予该终端），然后重新打开。"
        }
        "item" | "items" => "项",
        "Open Privacy Settings" => "打开隐私设置",
        "Windows kept {} {} unreadable. Run as administrator to read them{}." => {
            "Windows 使 {} {}无法读取。以管理员身份运行即可读取{}。"
        }
        ", and a whole drive several times faster" => "，而且整盘会快好几倍",
        "Run as administrator to read the whole drive from its file table: \
         several times faster than this walk." => {
            "以管理员身份运行可从文件表读取整个磁盘：比这样遍历快好几倍。"
        }
        " Restarting drops the marks." => " 重启会丢失标记。",
        "Restart as Administrator" => "以管理员身份重启",
        // ── the menu bar ────────────────────────────────────────────────
        "File" => "文件",
        "Go" => "前往",
        "Services" => "服务",
        "Hide disktree" => "隐藏 disktree",
        "Hide Others" => "隐藏其他",
        "Show All" => "全部显示",
        "Quit disktree" => "退出 disktree",
        "Open Folder…" => "打开文件夹…",
        "Rescan" => "重新扫描",
        "Close Window" => "关闭窗口",
        // ── ages ────────────────────────────────────────────────────────
        "unknown" => "未知",
        "just now" => "刚刚",
        "1 {} ago" => "1{}前",
        "{} {}s ago" => "{}{}前",
        "minute" => "分钟",
        "hour" => "小时",
        "day" => "天",
        "month" => "个月",
        "year" => "年",
        // ── what git knows ──────────────────────────────────────────────
        "clean" => "干净",
        "{} changed" => "{} 项改动",
        "changes unknown" => "改动未知",
        "no stash" => "无暂存",
        "1 stash" => "1 个暂存",
        "{} stashes" => "{} 个暂存",
        "{} unpushed" => "{} 个未推送",
        "no upstream" => "无上游",
        _ => return None,
    };
    Some(translated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_tags_name_a_language() {
        for tag in ["zh_CN.UTF-8", "zh-Hans-CN", "zh"] {
            assert_eq!(Locale::from_tag(tag), Locale::Zh, "{tag}");
        }
        for tag in ["en_US.UTF-8", "C", "de_DE", ""] {
            assert_eq!(Locale::from_tag(tag), Locale::En, "{tag}");
        }
    }

    #[test]
    fn an_untranslated_string_is_shown_as_it_is() {
        assert_eq!(translated(Locale::Zh, "disktree"), "disktree");
        assert_eq!(translated(Locale::En, "Selection"), "Selection");
    }

    #[test]
    fn the_screens_have_chinese() {
        assert_eq!(translated(Locale::Zh, "Selection"), "选择");
        assert_eq!(translated(Locale::Zh, "Worth a look"), "值得一看");
        assert_eq!(translated(Locale::Zh, "Move to trash"), "移到回收站");
        assert_eq!(
            translated(Locale::Zh, "{} marked · {} to free"),
            "已标记 {} 项 · 可释放 {}"
        );
    }

    #[test]
    fn placeholders_are_filled_in_order() {
        assert_eq!(
            substitute("{} 项匹配{} · {}", &[&"3", &"", &"1 GiB"]),
            "3 项匹配 · 1 GiB"
        );
        // A value that itself holds `{}` is data, not a placeholder.
        assert_eq!(substitute("[{}] {}", &[&"{}", &"x"]), "[{}] x");
    }

    #[test]
    fn a_saved_choice_survives_and_bad_data_is_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("disktree/language");
        assert_eq!(load(&path).expect("missing"), Language::System);
        for language in Language::ALL {
            save(&path, language).expect("save");
            assert_eq!(load(&path).expect("load"), language);
        }
        std::fs::write(&path, "klingon").expect("corrupt");
        assert_eq!(
            load(&path).expect_err("invalid").kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn the_system_choice_resolves_to_a_concrete_language() {
        assert_eq!(Language::English.resolve(), Locale::En);
        assert_eq!(Language::Chinese.resolve(), Locale::Zh);
        assert!(matches!(
            Language::System.resolve(),
            Locale::En | Locale::Zh
        ));
    }
}
