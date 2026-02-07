use super::{LanguageConfig, TestPattern};

pub const SYMBOLS_QUERY: &str = r#"
(function_definition
  name: (identifier) @function.name) @function.def

(class_definition
  name: (identifier) @class.name
  body: (block
    (function_definition
      name: (identifier) @method.name) @method.def)?) @class.def
"#;

pub fn config() -> LanguageConfig {
    LanguageConfig {
        language: tree_sitter_python::LANGUAGE.into(),
        symbols_query: SYMBOLS_QUERY,
        test_patterns: vec![TestPattern::FunctionPrefix("test_")],
    }
}
