//! Converts InstantReplay/Assets/Langs/*.axaml (App.axaml.cs / Core/Localizer.cs).
//! Run with `cargo run -p lag-core --example convert_langs -- [source-directory]`.

use std::{
    collections::BTreeMap,
    error::Error,
    path::{Path, PathBuf},
};

fn main() -> Result<(), Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("../../../InstantReplay/Assets/Langs"));
    let destination = root.join("langs");
    std::fs::create_dir_all(&destination)?;
    for entry in std::fs::read_dir(source)? {
        let path = entry?.path();
        if path.extension().is_none_or(|s| s != "axaml") {
            continue;
        }
        let xml = std::fs::read_to_string(&path)?;
        let document = roxmltree::Document::parse(&xml)?;
        let mut strings = BTreeMap::new();
        for node in document.descendants().filter(|n| n.is_element() && n.tag_name().name() == "String") {
            let key = node
                .attribute(("http://schemas.microsoft.com/winfx/2006/xaml", "Key"))
                .ok_or("String without x:Key")?;
            // Do not trim, normalize XAML escapes, or interpret literal backslash sequences.
            let value: String = node.children().filter_map(|n| n.text().filter(|_| n.is_text())).collect();
            if strings.insert(key, value).is_some() {
                return Err(format!("duplicate {key}").into());
            }
        }
        let code = path.file_stem().ok_or("missing language code")?;
        std::fs::write(destination.join(code).with_extension("json"), serde_json::to_string_pretty(&strings)?)?;
        println!("{}: {} keys", code.to_string_lossy(), strings.len());
    }
    Ok(())
}
