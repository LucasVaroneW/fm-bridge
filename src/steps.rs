// Step definitions loaded from steps.toml at compile time.
// This is the single source of truth for step names, shapes, and block behavior.
// Uses include_str! + toml crate for zero-runtime TOML parsing.

use serde::Deserialize;

/// How a step's data is stored in the FM XML.
/// Determines which XML elements to read/write.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum StepShape {
    /// No content elements (Halt Script, End If, etc.)
    Plain,
    /// Single <Calculation><![CDATA[...]]></Calculation>
    Calculation,
    /// <Restore state="False"/> + <Calculation> (If, Else If)
    CalculationWithRestore,
    /// <Value><Calculation> + <Repetition> + <Name> (Set Variable)
    ValueCalcName,
    /// <Text>...</Text> (comment)
    Comment,
    /// <Set state="..."/> (Set Error Capture)
    SetState,
    /// <Title> + <Message> + <Button> (Show Custom Dialog)
    Dialog,
    /// <Result> + <TargetName> (Set Field By Name)
    FieldByName,
    /// <ObjectName> + <FunctionName> + <P> (Perform JavaScript in Web Viewer)
    WebViewerJs,
    /// <Calculation> + <Field table="..." id="..." name="..."/> (Set Field)
    FieldAndCalc,
    /// <Script id="..." name="..."/> + optional <Calculation> + <CurrentScript value="..."/>
    /// Used by Perform Script and Perform Script on Server.
    PerformScript,
    /// <NoInteract state="..."/> + optional <Exit state="..."/> + <RowPageLocation value="..."/>
    /// + optional <Calculation> (for byCalculation). Used by Go to Record/Request/Page.
    GoToRecord,
    /// <SelectAll state="..."/> + <Calculation> + <Text/> + <Field>$varName</Field>
    /// Used by Execute FileMaker Data API. The Field element here carries a variable
    /// name as text content (not as a name attribute like Set Field).
    DataApi,
    /// <LimitToWindowsOfCurrentFile/> + <Window value="ByName|Current|First|.../> + <Name><Calculation>...</Calculation></Name>
    SelectWindow,
    /// <WindowState value="ResizeToFit|Maximize|Minimize|Restore|Hide"/>
    AdjustWindow,
    /// <ObjectName><Calculation>name</Calculation></ObjectName>
    /// + optional <Repetition><Calculation>n</Calculation></Repetition>
    /// Used by Go to Object.
    GoToObject,
    /// <LayoutDestination value="SelectedLayout|OriginalLayout"/>
    /// + optional <Layout name="..."/> (name-only — FM resolves on paste).
    /// Used by Go to Layout.
    GoToLayoutNamed,
    /// New Window: layout target + geometry calcs + window style name.
    /// FM also has bitfield style flags (Close/Resize/etc) which we default to
    /// the standard "Document" set on encode.
    NewWindow,
    /// <Restore state="True"/> + <Query> with N <RequestRow> each having Include/Omit
    /// and N <Criteria> (Field by table+name + Text). No numeric IDs are emitted —
    /// FM resolves field references by table+name on paste.
    PerformFind,
    /// <NoInteract/> + <DontEncodeURL/> + <SelectAll/> + <VerifySSLCertificates/>
    /// + <CURLOptions><Calculation>...</Calculation></CURLOptions>
    /// + <Calculation>url</Calculation> + <Text/> + <Field>$var</Field> or <Field name=.../>.
    /// Used by Insert from URL.
    InsertFromUrl,
    /// Whole inner XML preserved verbatim — no structured parsing.
    /// Used by steps whose FM config is too rich for a flat text line
    /// (Import/Export Records: <Profile>, <TargetFields> with N field maps, etc.).
    /// The raw inner XML round-trips losslessly via the `calculation` field.
    Opaque,
}

/// Internal step kind identifier from steps.toml.
/// Used for grouping steps with the same behavior.
#[derive(Debug, Clone, Deserialize)]
pub struct StepDef {
    /// Internal kind name (e.g. "SetVariable", "If")
    /// Reserved for future use (grouping, filtering, etc.)
    #[allow(dead_code)]
    pub kind: String,
    /// FileMaker global step type ID. None = not yet discovered.
    /// Use `fm-bridge dump-ids` to discover by copying a step in FM.
    #[serde(default)]
    pub id: Option<u32>,
    /// English name — canonical name used in text output and XML
    pub en: String,
    /// Spanish name — only for decode-side translation ES→EN
    #[serde(default)]
    pub es: String,
    /// How this step stores data in XML
    pub shape: StepShape,
    /// Whether this step increases indent level (If, Loop)
    #[serde(default)]
    pub opens_block: bool,
    /// Whether this step decreases indent level (End If, End Loop)
    #[serde(default)]
    pub closes_block: bool,
}

/// The full steps table, loaded at compile time from steps.toml.
#[derive(Deserialize)]
struct StepsFile {
    steps: Vec<StepDef>,
}

/// Static steps table — parsed once at compile time.
/// Using include_str! means the TOML is embedded in the binary,
/// so there's no file I/O at runtime.
static STEPS_JSON: &str = include_str!("../steps.toml");

fn parse_steps() -> Vec<StepDef> {
    let file: StepsFile = toml::from_str(STEPS_JSON)
        .expect("Failed to parse steps.toml at compile time");
    file.steps
}

/// Returns all step definitions.
pub fn all_steps() -> &'static [StepDef] {
    use std::sync::OnceLock;
    static STEPS: OnceLock<Vec<StepDef>> = OnceLock::new();
    STEPS.get_or_init(parse_steps)
}

/// Look up a step definition by its English name.
/// Returns None if the name is not recognized.
pub fn lookup_by_en(name: &str) -> Option<&'static StepDef> {
    all_steps().iter().find(|s| s.en == name)
}

/// Look up a step definition by its Spanish name.
/// Used during decode to translate FM Spanish names to English.
/// Returns None if no Spanish mapping exists.
pub fn lookup_by_es(name: &str) -> Option<&'static StepDef> {
    all_steps().iter().find(|s| !s.es.is_empty() && s.es == name)
}

/// Translate a step name to English.
/// If the name is already English, returns it as-is.
/// If it's a known Spanish name, returns the English equivalent.
/// If unknown, returns the name unchanged (caller should handle).
pub fn translate_to_en(name: &str) -> String {
    if let Some(def) = lookup_by_en(name) {
        return def.en.clone();
    }
    if let Some(def) = lookup_by_es(name) {
        return def.en.clone();
    }
    // Unknown step — return as-is, caller will add TODO comment
    name.to_string()
}

/// Check if a step name is a recognized English name.
#[allow(dead_code)]
pub fn is_known_en(name: &str) -> bool {
    lookup_by_en(name).is_some()
}

/// Get the StepShape for a known English step name.
/// Returns None if the name is not recognized.
pub fn shape_for_en(name: &str) -> Option<&'static StepShape> {
    lookup_by_en(name).map(|s| &s.shape)
}

/// Get the FileMaker step type ID for a known English step name.
/// Returns None if the name is unknown OR the id is not yet recorded in steps.toml.
pub fn id_for_en(name: &str) -> Option<u32> {
    lookup_by_en(name).and_then(|s| s.id)
}

/// Check if a step opens a block (increases indent).
pub fn opens_block(name: &str) -> bool {
    lookup_by_en(name).map(|s| s.opens_block).unwrap_or(false)
}

/// Check if a step closes a block (decreases indent).
pub fn closes_block(name: &str) -> bool {
    lookup_by_en(name).map(|s| s.closes_block).unwrap_or(false)
}

/// The canonical comment step name.
pub const COMMENT_NAME: &str = "# (comment)";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_translate_es_to_en() {
        assert_eq!(translate_to_en("Establecer variable"), "Set Variable");
        assert_eq!(translate_to_en("Si"), "If");
        assert_eq!(translate_to_en("Fin Si"), "End If");
        assert_eq!(translate_to_en("Mostrar cuadro de diálogo personalizado"), "Show Custom Dialog");
    }

    #[test]
    fn test_translate_en_stays_en() {
        assert_eq!(translate_to_en("Set Variable"), "Set Variable");
        assert_eq!(translate_to_en("If"), "If");
        assert_eq!(translate_to_en("End If"), "End If");
    }

    #[test]
    fn test_unknown_name_passthrough() {
        assert_eq!(translate_to_en("SomeUnknownStep"), "SomeUnknownStep");
    }

    #[test]
    fn test_block_behavior() {
        assert!(opens_block("If"));
        assert!(opens_block("Loop"));
        assert!(!opens_block("Set Variable"));

        assert!(closes_block("End If"));
        assert!(closes_block("End Loop"));
        assert!(!closes_block("If"));
    }

    #[test]
    fn test_shapes() {
        assert_eq!(shape_for_en("Set Variable"), Some(&StepShape::ValueCalcName));
        assert_eq!(shape_for_en("If"), Some(&StepShape::CalculationWithRestore));
        assert_eq!(shape_for_en("Halt Script"), Some(&StepShape::Plain));
        assert_eq!(shape_for_en("Show Custom Dialog"), Some(&StepShape::Dialog));
        assert_eq!(shape_for_en("# (comment)"), Some(&StepShape::Comment));
    }
}
