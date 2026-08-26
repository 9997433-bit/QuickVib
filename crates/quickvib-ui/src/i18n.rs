//! The window's language: Simplified Chinese by default, English on request.
//!
//! Every fixed string the window can draw is a [`Label`] with both spellings side by side in
//! one table, so a missing translation is a compile error rather than a stray English word on
//! a factory floor. Nothing here draws anything and nothing here needs a display, which is
//! what lets the whole vocabulary — and the number and unit formatting that goes with it — be
//! tested in the ordinary `cargo test` run.
//!
//! Chinese is not a fallback. [`Lang::default`] is [`Lang::Zh`], a first launch with no stored
//! preference is Chinese, and a stored preference that will not parse is Chinese.

use std::fmt;
use std::path::{Path, PathBuf};

use quickvib_core::{BackendKind, ExportFormat, SampleUnit, ScpiError};
use quickvib_engine::State;

use crate::form::Issue;

/// The language the window is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Lang {
    /// Simplified Chinese — the default everywhere.
    #[default]
    Zh,
    /// English.
    En,
}

impl Lang {
    /// The two-letter tag stored in the preference file.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Zh => "zh",
            Self::En => "en",
        }
    }

    /// Parse a stored tag. Anything unrecognised is Chinese, by policy.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        if text.trim().eq_ignore_ascii_case("en") {
            Self::En
        } else {
            Self::Zh
        }
    }

    /// The name of this language, written in that language: the label of its own button.
    #[must_use]
    pub const fn endonym(self) -> &'static str {
        match self {
            Self::Zh => "中文",
            Self::En => "EN",
        }
    }

    /// The fixed string for `label`.
    #[must_use]
    pub const fn t(self, label: Label) -> &'static str {
        match self {
            Self::Zh => label.zh(),
            Self::En => label.en(),
        }
    }
}

impl fmt::Display for Lang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Declare the label table: one line per string, Chinese first because Chinese is the default.
macro_rules! labels {
    ($( $variant:ident => $zh:literal, $en:literal ; )*) => {
        /// Every fixed string the window can show.
        ///
        /// The variants are generated from the table in this module; each one's documentation
        /// is the pair of spellings it stands for.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Label {
            $(
                #[doc = concat!("`", $zh, "` / `", $en, "`")]
                $variant,
            )*
        }

        impl Label {
            /// Every label, so a test can walk the whole vocabulary.
            pub const ALL: &'static [Label] = &[ $( Label::$variant, )* ];

            /// The Simplified Chinese spelling.
            #[must_use]
            pub const fn zh(self) -> &'static str {
                match self { $( Self::$variant => $zh, )* }
            }

            /// The English spelling.
            #[must_use]
            pub const fn en(self) -> &'static str {
                match self { $( Self::$variant => $en, )* }
            }
        }
    };
}

labels! {
    // ── Title bar ──────────────────────────────────────────────────────────────────────
    Tagline => "激光测振仪控制台", "Laser vibrometer console";
    Language => "语言", "Language";
    Theme => "主题", "Theme";
    ThemeLight => "浅色", "Light";
    ThemeDark => "深色", "Dark";
    LinkState => "连接状态", "Link";
    RecordState => "录制状态", "Record";
    Connected => "已连接", "Connected";
    Disconnected => "未连接", "No device";
    ListeningPorts => "实际监听端口", "Live listening ports";
    ScpiShort => "SCPI", "SCPI";
    DeviceShort => "设备", "Device";

    // ── Menus, tabs and file actions ───────────────────────────────────────────────────
    MenuFile => "文件", "File";
    TabSetup => "配置", "Setup";
    TabAdvanced => "高级", "Advanced";
    ActionOpen => "打开", "Open";
    ActionSave => "保存", "Save";
    ActionSaveAs => "另存为", "Save as";
    ActionApply => "应用", "Apply";
    ActionRevert => "还原", "Revert";
    ActionQuit => "退出", "Quit";
    ActionCancel => "取消", "Cancel";
    ActionStart => "开始录制", "Start record";
    ActionStop => "停止", "Stop";
    ActionExport => "导出数据", "Export capture";

    // ── Configuration sections ─────────────────────────────────────────────────────────
    SectionProject => "项目信息", "Project";
    SectionAcquisition => "设备与采样", "Device and sampling";
    SectionFilters => "滤波器", "Filters";
    SectionRanges => "量程", "Measuring ranges";
    SectionRecording => "录制", "Recording";
    SectionPorts => "通信端口", "I/O ports";
    SectionExport => "数据导出", "Export";

    // ── Configuration fields ───────────────────────────────────────────────────────────
    FieldProjectName => "项目名称", "Project name";
    FieldDescription => "说明", "Description";
    FieldSampleRate => "采样率", "Sample rate";
    FieldDataType => "数据类型", "Data type";
    FieldBackend => "数据来源", "Backend";
    FieldDuration => "录制时长", "Record duration";
    FieldLowPass => "低通滤波", "Low-pass filter";
    FieldHighPass => "高通滤波", "High-pass filter";
    FieldPinCutoff => "固定截止", "Pin cutoff";
    FieldVelocityRange => "速度量程", "Velocity range";
    FieldDisplacementRange => "位移量程", "Displacement range";
    FieldAccelerationRange => "加速度量程", "Acceleration range";
    FieldScpiPort => "SCPI 端口", "SCPI port";
    FieldDevicePort => "设备端口", "Device port";
    FieldProjectPorts => "项目端口（需重启）", "Project ports (restart required)";
    FieldExportFormat => "导出格式", "Format";
    FieldExportDirectory => "导出目录", "Directory";
    FieldExportOptions => "导出选项", "Options";
    FieldFileName => "文件名", "File name";
    OptionCsvHeader => "写入 CSV 表头与前导信息", "CSV preamble and header row";
    OptionRemoveDc => "计算前扣除直流分量", "Remove DC before peak and RMS";

    // ── Hints ──────────────────────────────────────────────────────────────────────────
    HintFilterPairing => "采样率与低通须同档", "Sample rate and low-pass must be set as a matched pair";
    HintNyquist => "未固定时，低通截止自动跟随奈奎斯特频率（采样率的一半）",
        "Unpinned, the low-pass cutoff follows the Nyquist frequency of the sample rate";
    HintHighPassOff => "填 0 表示关闭", "0 disables";
    HintScpiPeer => "测试系统接入", "the UTS connects in";
    HintDevicePeer => "M300 接入", "the M300 dials in";
    HintLivePorts => "本进程已绑定的端口，命令行参数优先于项目文件",
        "The ports this process actually bound; the command line wins over the project file";
    HintProjectPorts => "端口写入工程文件，下次启动生效，不影响当前监听",
        "Ports are written to the project file and take effect at the next start";
    HintPortMismatch => "项目端口与当前监听端口不一致：本次启动使用的是命令行端口",
        "The project ports differ from the live ports: this run was started with command-line ports";
    HintRunningLock => "录制进行中：停止后方可修改配置",
        "A recording is in flight: stop it to edit the configuration";
    HintExportDirectory => "相对路径基于此目录", "relative export paths resolve here";

    // ── Instrument panel ───────────────────────────────────────────────────────────────
    SectionInstrument => "仪表面板", "Instrument";
    SectionMeasurements => "测量结果", "Measurements";
    SectionLink => "链路信息", "Link details";
    MeasPeak => "峰值", "Peak";
    MeasRms => "有效值", "RMS";
    MeasPeakToPeak => "峰峰值", "Peak-to-peak";
    MeasSamples => "样本数", "Samples";
    NoCapture => "尚无完成的采集", "No completed capture";
    LabelIdentity => "设备标识", "Identity";
    LabelPeer => "设备对端", "Device peer";
    LabelSessions => "SCPI 会话", "SCPI sessions";
    LabelPendingErrors => "错误队列", "Errors queued";
    LabelBackend => "数据来源", "Backend";
    LabelDuration => "时长", "Duration";
    LabelNone => "无", "none";
    LabelUnsavedProject => "（未保存的项目）", "(unsaved project)";
    LabelModified => "已修改", "modified";
    LabelVersion => "版本", "Version";

    // ── Dialogs, notices and errors ────────────────────────────────────────────────────
    DialogOpen => "打开项目", "Open project";
    DialogSaveAs => "项目另存为", "Save project as";
    DialogPath => ".proj 工程文件路径", "Path to a .proj file";
    NoticeReady => "就绪", "Ready";
    NoticeApplied => "已应用到运行中的仪器", "Applied to the running instrument";
    NoticeReverted => "已还原为已加载的项目", "Reverted to the loaded project";
    NoticeStarted => "录制已开始", "Recording started";
    NoticeStopRequested => "已请求停止", "Abort requested";
    NoticeEmptyPath => "请先输入工程文件路径", "Enter a project path first";
    NoticeEmptyFileName => "请先输入导出文件名", "Enter a file name to export to";
    ErrorsHeading => "被拒绝的字段", "Rejected fields";
    RestartHeading => "需重启后生效", "Restart QuickVib for";

    // ── Advanced tab ───────────────────────────────────────────────────────────────────
    AdvancedTitle => "高级设备控制", "Advanced device controls";
    AdvancedBody => "激光功率、TEC 温控设定点、PID 环路增益与外部触发都是设备侧控制项，\
        版本 1 的工程文件格式中没有对应字段。QuickVib 不会凭空造出仪器无法执行的设置。",
        "Laser power, TEC set point, PID loop gains and external triggering are device controls \
        with no representation in the version-1 project schema, and QuickVib does not invent \
        settings the instrument cannot honour.";
    AdvancedBody2 => "待工程文件格式与 M300 后端支持这些参数后，它们会出现在本页；在此之前，\
        「配置」页就是全部的配置界面。",
        "They land here once the schema and the M300 backend carry them; until then the Setup \
        tab is the whole configuration surface.";
}

/// The Chinese/English name of an instrument state: 空闲 / 武装 / 录制中 / 完成 / 已中止.
#[must_use]
pub const fn state_label(lang: Lang, state: State) -> &'static str {
    match (lang, state) {
        (Lang::Zh, State::Idle) => "空闲",
        (Lang::Zh, State::Armed) => "武装",
        (Lang::Zh, State::Recording) => "录制中",
        (Lang::Zh, State::Complete) => "完成",
        (Lang::Zh, State::Aborted) => "已中止",
        (Lang::En, _) => state.as_scpi_str(),
    }
}

/// The Chinese/English name of a backend: 模拟 / TCP设备 / M300.
#[must_use]
pub const fn backend_label(lang: Lang, backend: BackendKind) -> &'static str {
    match (lang, backend) {
        (Lang::Zh, BackendKind::Mock) => "模拟",
        (Lang::Zh, BackendKind::Tcp) => "TCP设备",
        (Lang::En, BackendKind::Mock) => "Simulated",
        (Lang::En, BackendKind::Tcp) => "TCP device",
        (_, BackendKind::M300) => "M300",
    }
}

/// The physical symbol of a sample unit, in the spelling an operator expects on screen:
/// `μm/s`, `μm`, `m/s²`. Deliberately not [`SampleUnit::label`], which is the ASCII spelling
/// the exported files carry.
#[must_use]
pub const fn unit_symbol(unit: SampleUnit) -> &'static str {
    match unit {
        SampleUnit::VelocityUmPerSec => "μm/s",
        SampleUnit::DisplacementUm => "μm",
        SampleUnit::AccelerationMPerSec2 => "m/s²",
    }
}

/// The name of a measured quantity with its unit: 速度 μm/s, 位移 μm, 加速度 m/s².
#[must_use]
pub fn unit_label(lang: Lang, unit: SampleUnit) -> String {
    let name = match (lang, unit) {
        (Lang::Zh, SampleUnit::VelocityUmPerSec) => "速度",
        (Lang::Zh, SampleUnit::DisplacementUm) => "位移",
        (Lang::Zh, SampleUnit::AccelerationMPerSec2) => "加速度",
        (Lang::En, SampleUnit::VelocityUmPerSec) => "Velocity",
        (Lang::En, SampleUnit::DisplacementUm) => "Displacement",
        (Lang::En, SampleUnit::AccelerationMPerSec2) => "Acceleration",
    };
    format!("{name} {}", unit_symbol(unit))
}

/// The label of an export format. The formats are file-format names, so both languages spell
/// them the same way; the function exists so the window never has to special-case them.
#[must_use]
pub const fn format_label(format: ExportFormat) -> &'static str {
    format.as_scpi_str()
}

/// A frequency in the unit a technician reads it in: `100 kHz`, `48 kHz`, `500 Hz`, `2 MHz`.
///
/// Only the display changes — the field an operator edits stays in hertz, because that is
/// what the project file and the SCPI surface carry.
#[must_use]
pub fn frequency(hz: f64) -> String {
    if !hz.is_finite() || hz <= 0.0 {
        return "-".to_owned();
    }
    let (scaled, unit) = if hz >= 1e6 {
        (hz / 1e6, "MHz")
    } else if hz >= 1e3 {
        (hz / 1e3, "kHz")
    } else {
        (hz, "Hz")
    };
    format!("{} {unit}", trim_number(scaled))
}

/// A duration with its unit: `1 秒` / `1 s`.
#[must_use]
pub fn seconds(lang: Lang, value: f64) -> String {
    match lang {
        Lang::Zh => format!("{} 秒", trim_number(value)),
        Lang::En => format!("{} s", trim_number(value)),
    }
}

/// A measured value at the four decimals the panel shows, with the unit of the loaded
/// project. QuickVib performs no unit conversion, so the unit is whatever the device is
/// configured for — and there is none to show until a project says what that is.
#[must_use]
pub fn measurement(value: f64, unit: Option<SampleUnit>) -> String {
    match unit {
        Some(unit) => format!("{value:.4} {}", unit_symbol(unit)),
        None => format!("{value:.4}"),
    }
}

/// `n` field(s) rejected, as one sentence.
#[must_use]
pub fn rejected_fields(lang: Lang, count: usize) -> String {
    match lang {
        Lang::Zh => format!("{count} 项设置被拒绝"),
        Lang::En => format!("{count} field(s) rejected"),
    }
}

/// "Saved <path>", localised.
#[must_use]
pub fn saved(lang: Lang, path: &Path) -> String {
    match lang {
        Lang::Zh => format!("已保存 {}", path.display()),
        Lang::En => format!("saved {}", path.display()),
    }
}

/// "Loaded <path>", localised.
#[must_use]
pub fn loaded(lang: Lang, path: &str) -> String {
    match lang {
        Lang::Zh => format!("已加载 {path}"),
        Lang::En => format!("loaded {path}"),
    }
}

/// "Wrote <path>", localised — the outcome of an export.
#[must_use]
pub fn wrote(lang: Lang, path: &Path) -> String {
    match lang {
        Lang::Zh => format!("已写入 {}", path.display()),
        Lang::En => format!("wrote {}", path.display()),
    }
}

/// The SCPI-99 error text in `lang`. The numeric code is never translated — it is what a
/// technician looks up in the manual and what the UTS log shows.
#[must_use]
pub const fn scpi_message(lang: Lang, error: ScpiError) -> &'static str {
    match lang {
        Lang::En => error.message(),
        Lang::Zh => match error {
            ScpiError::NoError => "无错误",
            ScpiError::CommandError => "命令错误",
            ScpiError::UndefinedHeader => "未知命令",
            ScpiError::SettingsConflict => "设置冲突",
            ScpiError::DataOutOfRange => "数据超出范围",
            ScpiError::IllegalParameterValue => "参数非法",
            ScpiError::DataCorruptOrStale => "数据无效或已过期",
            ScpiError::HardwareError => "硬件错误",
            ScpiError::HardwareMissing => "未检测到设备",
            ScpiError::FileNameNotFound => "文件不存在",
            ScpiError::FileNameError => "文件路径错误",
            ScpiError::QueueOverflow => "错误队列溢出",
            ScpiError::TimeoutError => "操作超时",
            // The catalogue is `#[non_exhaustive]`: a code added upstream shows its English
            // name rather than nothing at all.
            _ => error.message(),
        },
    }
}

/// A refusal, as the status bar shows it: what was attempted, why, and the SCPI code.
#[must_use]
pub fn refusal(lang: Lang, attempt: &str, error: ScpiError) -> String {
    match lang {
        Lang::Zh => format!(
            "{attempt}：{}（{}）",
            scpi_message(lang, error),
            error.code()
        ),
        Lang::En => format!(
            "{attempt}: {} ({})",
            scpi_message(lang, error),
            error.code()
        ),
    }
}

/// The operator-facing name of a schema field path, e.g. `device.sampleRateHz` → 采样率.
///
/// A dotted schema path is a developer's name for a setting, not a technician's, and it must
/// never be the only thing an error message says. Paths the table does not know fall back to
/// the path itself, which is still better than nothing and shows up immediately in review.
#[must_use]
pub fn field_label(lang: Lang, field: &str) -> &str {
    let label = match field {
        "name" => Label::FieldProjectName,
        "description" => Label::FieldDescription,
        "device.sampleRateHz" => Label::FieldSampleRate,
        "device.unit" => Label::FieldDataType,
        "device.backend" => Label::FieldBackend,
        "device.lpfHz" => Label::FieldLowPass,
        "device.highPassHz" => Label::FieldHighPass,
        "device.velocityRange" => Label::FieldVelocityRange,
        "device.displacementRange" => Label::FieldDisplacementRange,
        "device.accelerationRange" => Label::FieldAccelerationRange,
        "device.port" => Label::FieldDevicePort,
        "recording.durationSeconds" => Label::FieldDuration,
        "server.scpiPort" => Label::FieldScpiPort,
        "export.directory" => Label::FieldExportDirectory,
        "export.format" => Label::FieldExportFormat,
        "project" => Label::SectionProject,
        _ => return field,
    };
    lang.t(label)
}

/// The reason a field was rejected, in `lang`.
///
/// `None` for [`Issue::Schema`], the one rejection that has no structure behind it: the
/// caller falls back to the validator's own English sentence rather than invent a translation
/// for a rule the form does not model.
#[must_use]
pub fn issue_text(lang: Lang, issue: &Issue) -> Option<String> {
    let text = match (lang, issue) {
        (_, Issue::Schema) => return None,

        (Lang::Zh, Issue::Empty) => "不能为空".to_owned(),
        (Lang::Zh, Issue::NotANumber { input }) => format!("「{input}」不是有效数字"),
        (Lang::Zh, Issue::NotPositive { value }) => {
            format!("必须大于零（当前 {}）", trim_number(*value))
        }
        (Lang::Zh, Issue::Negative { value }) => {
            format!("不能为负数（当前 {}）", trim_number(*value))
        }
        (Lang::Zh, Issue::BadPort { input }) => {
            format!("「{input}」不是 1–65535 之间的端口号")
        }
        (Lang::Zh, Issue::DuplicatePort) => "必须与设备端口不同".to_owned(),
        (Lang::Zh, Issue::AboveLowPass { lpf_hz }) => {
            format!("必须低于低通截止频率（{}）", frequency(*lpf_hz))
        }
        (Lang::Zh, Issue::DurationTooLong { max_seconds }) => {
            format!("不得超过 {} 秒", trim_number(*max_seconds))
        }
        (Lang::Zh, Issue::CaptureTooLarge { max_bytes }) => {
            format!("采集数据量超过上限（{max_bytes} 字节），请缩短时长或降低采样率")
        }
        (Lang::Zh, Issue::RunInFlight) => "录制进行中：请先停止再修改配置".to_owned(),

        (Lang::En, Issue::Empty) => "must not be empty".to_owned(),
        (Lang::En, Issue::NotANumber { input }) => format!("'{input}' is not a finite number"),
        (Lang::En, Issue::NotPositive { value }) => {
            format!("must be greater than zero, found {}", trim_number(*value))
        }
        (Lang::En, Issue::Negative { value }) => {
            format!("must not be negative, found {}", trim_number(*value))
        }
        (Lang::En, Issue::BadPort { input }) => format!("'{input}' is not a port in 1..=65535"),
        (Lang::En, Issue::DuplicatePort) => "must differ from the device port".to_owned(),
        (Lang::En, Issue::AboveLowPass { lpf_hz }) => {
            format!("must be below the low-pass cutoff ({})", frequency(*lpf_hz))
        }
        (Lang::En, Issue::DurationTooLong { max_seconds }) => {
            format!("must not exceed {} s", trim_number(*max_seconds))
        }
        (Lang::En, Issue::CaptureTooLarge { max_bytes }) => format!(
            "the capture would exceed the {max_bytes} byte cap; shorten it or sample slower"
        ),
        (Lang::En, Issue::RunInFlight) => {
            "a recording is in flight; stop it before applying changes".to_owned()
        }
    };
    Some(text)
}

/// Shortest round-tripping decimal form: `100` rather than `100.0`, `2.5` unchanged.
fn trim_number(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

/// Punctuation and symbols the window composes its own strings from — the separator between
/// two readings, the menu arrow, the equals sign in front of a converted unit.
///
/// Listed in one place because the embedded font is a subset: the coverage test walks this
/// string along with the label table, so a decoration the font has no glyph for fails
/// `cargo test` instead of showing up as a box on the instrument. Anything the window draws
/// that is not a [`Label`] belongs here.
pub const CHROME_PUNCTUATION: &str = "·=:▼";

/// File name of the language preference inside the QuickVib state directory.
pub const LANG_FILE_NAME: &str = "ui-language.txt";

/// Where the window remembers the operator's language choice between launches.
///
/// The record lives beside the auto-load-last project record, in the same per-user state
/// directory (`%LOCALAPPDATA%\QuickVib\` on Windows, `$XDG_STATE_HOME/quickvib/` elsewhere),
/// and every failure to read or write it is silent: a locked-down host that cannot keep the
/// preference simply opens in Chinese, which is the default anyway.
#[derive(Debug, Clone)]
pub struct LangStore {
    path: PathBuf,
}

impl LangStore {
    /// A store that keeps its record in `dir`.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            path: dir.into().join(LANG_FILE_NAME),
        }
    }

    /// A store in the platform state directory, when one can be resolved.
    #[must_use]
    pub fn discover() -> Option<Self> {
        quickvib_project::last_project::state_dir().map(Self::new)
    }

    /// The full path of the record.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The stored preference, or `None` when nothing has been stored yet.
    #[must_use]
    pub fn read(&self) -> Option<Lang> {
        std::fs::read_to_string(&self.path)
            .ok()
            .map(|text| Lang::parse(&text))
    }

    /// Remember `lang`. Returns whether the record could be written.
    pub fn write(&self, lang: Lang) -> bool {
        let Some(parent) = self.path.parent() else {
            return false;
        };
        if std::fs::create_dir_all(parent).is_err() {
            return false;
        }
        std::fs::write(&self.path, lang.as_str()).is_ok()
    }
}

/// The language the window opens in: the stored preference when there is one, Chinese
/// otherwise.
#[must_use]
pub fn startup_lang(store: Option<&LangStore>) -> Lang {
    store.and_then(LangStore::read).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn chinese_is_the_default_everywhere() {
        assert_eq!(Lang::default(), Lang::Zh);
        assert_eq!(startup_lang(None), Lang::Zh);
        assert_eq!(Lang::parse(""), Lang::Zh);
        assert_eq!(Lang::parse("de"), Lang::Zh);
        assert_eq!(Lang::parse("EN"), Lang::En);
        assert_eq!(Lang::parse(" en\n"), Lang::En);
    }

    #[test]
    fn every_label_has_both_spellings() {
        for label in Label::ALL {
            assert!(!label.zh().trim().is_empty(), "{label:?} has no Chinese");
            assert!(!label.en().trim().is_empty(), "{label:?} has no English");
        }
    }

    #[test]
    fn every_chinese_label_actually_contains_chinese() {
        // A translation that was left in English is the failure mode this table exists to
        // prevent, so every entry must carry at least one CJK ideograph — except the handful
        // of protocol and product names that are spelled the same in both languages.
        let same_in_both = [Label::ScpiShort];
        for label in Label::ALL {
            if same_in_both.contains(label) {
                continue;
            }
            assert!(
                label.zh().chars().any(is_cjk),
                "{label:?} is still English: {}",
                label.zh()
            );
        }
    }

    #[test]
    fn the_default_language_reaches_every_lookup() {
        let lang = Lang::default();
        assert_eq!(lang.t(Label::ActionStart), "开始录制");
        assert_eq!(lang.t(Label::ActionStop), "停止");
        assert_eq!(lang.t(Label::MeasPeak), "峰值");
        assert_eq!(lang.t(Label::MeasRms), "有效值");
        assert_eq!(lang.t(Label::MeasPeakToPeak), "峰峰值");
        assert_eq!(lang.t(Label::ActionOpen), "打开");
        assert_eq!(lang.t(Label::ActionSave), "保存");
        assert_eq!(lang.t(Label::ActionSaveAs), "另存为");
        assert_eq!(lang.t(Label::ActionApply), "应用");
        assert_eq!(lang.t(Label::ActionRevert), "还原");
        assert_eq!(lang.t(Label::HintFilterPairing), "采样率与低通须同档");
    }

    #[test]
    fn english_is_available_for_every_label() {
        assert_eq!(Lang::En.t(Label::ActionStart), "Start record");
        assert_eq!(Lang::En.t(Label::MeasRms), "RMS");
    }

    #[test]
    fn states_are_named_in_chinese() {
        let names: Vec<&str> = State::all()
            .iter()
            .map(|state| state_label(Lang::Zh, *state))
            .collect();
        assert_eq!(names, ["空闲", "武装", "录制中", "完成", "已中止"]);
        // English keeps the SCPI wire spelling, which is what the UTS logs show.
        assert_eq!(state_label(Lang::En, State::Recording), "RECORDING");
    }

    #[test]
    fn backends_are_named_in_chinese_not_in_schema_words() {
        assert_eq!(backend_label(Lang::Zh, BackendKind::Mock), "模拟");
        assert_eq!(backend_label(Lang::Zh, BackendKind::Tcp), "TCP设备");
        assert_eq!(backend_label(Lang::Zh, BackendKind::M300), "M300");
        for backend in [BackendKind::Mock, BackendKind::Tcp, BackendKind::M300] {
            assert_ne!(
                backend_label(Lang::Zh, backend),
                backend.as_str(),
                "the schema spelling must not reach the window"
            );
        }
    }

    #[test]
    fn units_are_shown_with_their_physical_symbols() {
        assert_eq!(
            unit_label(Lang::Zh, SampleUnit::VelocityUmPerSec),
            "速度 μm/s"
        );
        assert_eq!(unit_label(Lang::Zh, SampleUnit::DisplacementUm), "位移 μm");
        assert_eq!(
            unit_label(Lang::Zh, SampleUnit::AccelerationMPerSec2),
            "加速度 m/s²"
        );
        for unit in SampleUnit::all() {
            let shown = unit_label(Lang::Zh, *unit);
            assert!(
                !shown.contains('_'),
                "a schema spelling reached the window: {shown}"
            );
        }
    }

    #[test]
    fn frequencies_are_shown_in_the_unit_a_technician_reads() {
        assert_eq!(frequency(100_000.0), "100 kHz");
        assert_eq!(frequency(48_000.0), "48 kHz");
        assert_eq!(frequency(1_500.0), "1.5 kHz");
        assert_eq!(frequency(500.0), "500 Hz");
        assert_eq!(frequency(2_000_000.0), "2 MHz");
        assert_eq!(frequency(0.0), "-");
        assert_eq!(frequency(f64::NAN), "-");
    }

    #[test]
    fn durations_and_measurements_carry_their_units() {
        assert_eq!(seconds(Lang::Zh, 1.0), "1 秒");
        assert_eq!(seconds(Lang::En, 2.5), "2.5 s");
        assert_eq!(
            measurement(12.5, Some(SampleUnit::VelocityUmPerSec)),
            "12.5000 μm/s"
        );
        assert_eq!(measurement(12.5, None), "12.5000");
    }

    #[test]
    fn dynamic_sentences_are_localised() {
        assert_eq!(rejected_fields(Lang::Zh, 2), "2 项设置被拒绝");
        assert_eq!(rejected_fields(Lang::En, 2), "2 field(s) rejected");
        assert_eq!(
            saved(Lang::Zh, Path::new("/tmp/a.proj")),
            "已保存 /tmp/a.proj"
        );
        assert_eq!(loaded(Lang::Zh, "a.proj"), "已加载 a.proj");
        assert_eq!(wrote(Lang::Zh, Path::new("out.csv")), "已写入 out.csv");
    }

    #[test]
    fn rejected_fields_are_named_and_explained_in_chinese() {
        assert_eq!(field_label(Lang::Zh, "device.sampleRateHz"), "采样率");
        assert_eq!(
            field_label(Lang::Zh, "recording.durationSeconds"),
            "录制时长"
        );
        assert_eq!(
            field_label(Lang::En, "device.highPassHz"),
            "High-pass filter"
        );
        // An unknown path is shown as itself rather than swallowed.
        assert_eq!(field_label(Lang::Zh, "device.whatsit"), "device.whatsit");

        assert_eq!(issue_text(Lang::Zh, &Issue::Empty).unwrap(), "不能为空");
        assert_eq!(
            issue_text(Lang::Zh, &Issue::NotPositive { value: -1.0 }).unwrap(),
            "必须大于零（当前 -1）"
        );
        assert_eq!(
            issue_text(Lang::Zh, &Issue::AboveLowPass { lpf_hz: 50_000.0 }).unwrap(),
            "必须低于低通截止频率（50 kHz）"
        );
        assert_eq!(
            issue_text(
                Lang::Zh,
                &Issue::BadPort {
                    input: "70000".to_owned()
                }
            )
            .unwrap(),
            "「70000」不是 1–65535 之间的端口号"
        );
        // The one rejection with no structure behind it falls back to the validator's words.
        assert!(issue_text(Lang::Zh, &Issue::Schema).is_none());
    }

    #[test]
    fn the_preference_round_trips_through_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = LangStore::new(dir.path());
        assert_eq!(store.read(), None);
        assert_eq!(startup_lang(Some(&store)), Lang::Zh);

        assert!(store.write(Lang::En));
        assert_eq!(store.read(), Some(Lang::En));
        assert_eq!(startup_lang(Some(&store)), Lang::En);

        assert!(store.write(Lang::Zh));
        assert_eq!(startup_lang(Some(&store)), Lang::Zh);
        assert!(store.path().ends_with(LANG_FILE_NAME));
    }

    #[test]
    fn an_unwritable_store_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("occupied");
        std::fs::write(&file, "not a directory").unwrap();
        // `file` is a regular file, so the store cannot create its directory under it.
        let store = LangStore::new(file.join("state"));
        assert!(!store.write(Lang::En));
        assert_eq!(startup_lang(Some(&store)), Lang::Zh);
    }

    fn is_cjk(c: char) -> bool {
        matches!(c as u32, 0x3000..=0x303F | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF)
    }
}
