//! What a tool is: its arguments, its result and the path policy.

use serde_json::{Map, Value, json};
use std::path::{Component, Path, PathBuf};

/// One block of a tool result.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    /// Text (the tools put pretty JSON here).
    Text(String),
    /// A PNG image.
    Png(Vec<u8>),
}

/// The result of a tool call. A failed call (`is_error`) is still a result the model can read.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    /// The blocks, in order.
    pub content: Vec<Content>,
    /// The call failed.
    pub is_error: bool,
}

impl ToolOutput {
    /// A text result.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text(text.into())],
            is_error: false,
        }
    }

    /// A JSON result, pretty printed.
    pub fn json(value: &Value) -> Self {
        Self::text(serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()))
    }

    /// A failed call with a message.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text(message.into())],
            is_error: true,
        }
    }

    /// Adds an image block.
    #[must_use]
    pub fn with_png(mut self, png: Vec<u8>) -> Self {
        self.content.push(Content::Png(png));
        self
    }

    /// The first text block (tests).
    pub fn first_text(&self) -> &str {
        self.content
            .iter()
            .find_map(|c| match c {
                Content::Text(t) => Some(t.as_str()),
                Content::Png(_) => None,
            })
            .unwrap_or("")
    }

    /// The first text block parsed as JSON (tests).
    pub fn parsed(&self) -> Value {
        serde_json::from_str(self.first_text()).unwrap_or(Value::Null)
    }
}

/// The arguments of a call, read by name.
#[derive(Debug, Clone)]
pub struct Args(pub Map<String, Value>);

impl Args {
    /// From the `arguments` value of a call (anything but an object is empty).
    pub fn new(value: Option<&Value>) -> Self {
        Self(
            value
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
        )
    }

    /// A required string.
    ///
    /// # Errors
    /// A message naming the argument.
    pub fn str(&self, name: &str) -> Result<&str, String> {
        self.opt_str(name)?
            .ok_or_else(|| format!("missing argument `{name}`"))
    }

    /// An optional string.
    ///
    /// # Errors
    /// When it is present and not a string.
    pub fn opt_str(&self, name: &str) -> Result<Option<&str>, String> {
        match self.0.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s)),
            Some(_) => Err(format!("argument `{name}` must be a string")),
        }
    }

    /// An optional number.
    ///
    /// # Errors
    /// When it is present and not a number.
    pub fn opt_f64(&self, name: &str) -> Result<Option<f64>, String> {
        match self.0.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_f64()
                .map(Some)
                .ok_or_else(|| format!("argument `{name}` must be a number")),
        }
    }

    /// A required number.
    ///
    /// # Errors
    /// A message naming the argument.
    pub fn f64(&self, name: &str) -> Result<f64, String> {
        self.opt_f64(name)?
            .ok_or_else(|| format!("missing argument `{name}`"))
    }

    /// An optional non-negative whole number.
    ///
    /// # Errors
    /// When it is present and not one.
    pub fn opt_u64(&self, name: &str) -> Result<Option<u64>, String> {
        match self.0.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_u64()
                .map(Some)
                .ok_or_else(|| format!("argument `{name}` must be a non-negative whole number")),
        }
    }

    /// An optional boolean.
    ///
    /// # Errors
    /// When it is present and not a boolean.
    pub fn opt_bool(&self, name: &str) -> Result<Option<bool>, String> {
        match self.0.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(format!("argument `{name}` must be true or false")),
        }
    }

    /// An optional list of strings (empty when absent).
    ///
    /// # Errors
    /// When it is present and not a list of strings.
    pub fn opt_strings(&self, name: &str) -> Result<Vec<String>, String> {
        match self.0.get(name) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Array(a)) => a
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| format!("argument `{name}` must be a list of strings"))
                })
                .collect(),
            Some(_) => Err(format!("argument `{name}` must be a list of strings")),
        }
    }
}

/// A canonical path without the `\\?\` prefix Windows adds: the verbatim form reads badly in a
/// result and in a model's next command. UNC shares keep theirs.
fn plain(p: PathBuf) -> PathBuf {
    let text = p.to_string_lossy();
    match text.strip_prefix("\\\\?\\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => p,
    }
}

/// The folders the tools may read and write. A relative path is taken from the working
/// directory; every path must end up inside a root.
#[derive(Debug, Clone)]
pub struct Paths {
    cwd: PathBuf,
    roots: Vec<PathBuf>,
}

impl Paths {
    /// The working directory is a root; `extra` adds more.
    pub fn new(cwd: PathBuf, extra: &[PathBuf]) -> Self {
        let canon = |p: &Path| plain(std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
        let mut roots = vec![canon(&cwd)];
        roots.extend(extra.iter().map(|p| canon(&cwd.join(p))));
        Self { cwd, roots }
    }

    /// The roots, canonical.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    fn inside(&self, p: &Path) -> bool {
        self.roots.iter().any(|r| p.starts_with(r))
    }

    /// An existing file or folder to read.
    ///
    /// # Errors
    /// When it does not exist or lies outside every root.
    pub fn read(&self, name: &str) -> Result<PathBuf, String> {
        let joined = self.cwd.join(name);
        let canon = std::fs::canonicalize(&joined)
            .map(plain)
            .map_err(|e| format!("{}: cannot read: {e}", joined.display()))?;
        if self.inside(&canon) {
            Ok(canon)
        } else {
            Err(self.refusal(&canon))
        }
    }

    /// A file to write; its folder is created.
    ///
    /// # Errors
    /// When the file would lie outside every root, or its folder cannot be made.
    pub fn write(&self, name: &str) -> Result<PathBuf, String> {
        let joined = self.cwd.join(name);
        let file = joined
            .file_name()
            .ok_or_else(|| format!("{}: no file name", joined.display()))?
            .to_owned();
        let dir = joined.parent().unwrap_or(&self.cwd).to_path_buf();
        // Look before creating anything: a `..` must not leave a root through a folder that
        // does not exist yet.
        let mut existing = dir.clone();
        while !existing.exists() {
            if !existing.pop() {
                break;
            }
        }
        let canon_existing = std::fs::canonicalize(&existing)
            .map(plain)
            .map_err(|e| format!("{}: {e}", existing.display()))?;
        let tail = dir.strip_prefix(&existing).unwrap_or(Path::new(""));
        if tail.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(self.refusal(&dir));
        }
        let target_dir = canon_existing.join(tail);
        let target = target_dir.join(&file);
        if !self.inside(&target) {
            return Err(self.refusal(&target));
        }
        std::fs::create_dir_all(&target_dir)
            .map_err(|e| format!("{}: cannot create the folder: {e}", dir.display()))?;
        Ok(target)
    }

    /// Where outputs go when the caller names none: `<first root>/target/mcp/<name>`.
    pub fn default_output(&self, name: &str) -> String {
        self.roots[0]
            .join("target")
            .join("mcp")
            .join(name)
            .to_string_lossy()
            .into_owned()
    }

    fn refusal(&self, p: &Path) -> String {
        let roots: Vec<String> = self.roots.iter().map(|r| r.display().to_string()).collect();
        format!(
            "{}: outside the allowed folders ({}); start atomik-mcp with --root <folder> to allow more",
            p.display(),
            roots.join(", ")
        )
    }
}

/// A tool definition for `tools/list`: `properties` are `(name, type, description)` with the
/// type a JSON schema type or `strings` for a list of strings.
pub fn definition(
    name: &str,
    description: &str,
    properties: &[(&str, &str, &str)],
    required: &[&str],
) -> Value {
    let props: Map<String, Value> = properties
        .iter()
        .map(|(n, ty, desc)| {
            let schema = match *ty {
                "strings" => {
                    json!({"type": "array", "items": {"type": "string"}, "description": desc})
                }
                other => json!({"type": other, "description": desc}),
            };
            ((*n).to_string(), schema)
        })
        .collect();
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": props,
            "required": required,
            "additionalProperties": false
        }
    })
}
