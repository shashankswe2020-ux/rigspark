const BROWSER_SCRIPTS: &[&str] = &[
    "crates/rigspark-gui/static/calculator-runtime.js",
    "crates/rigspark-gui/static/calculator-template.js",
    "crates/rigspark-gui/static/chat.js",
    "crates/rigspark-gui/static/generate.js",
    "crates/rigspark-gui/static/markdown.js",
    "crates/rigspark-gui/static/run-reducer.js",
    "crates/rigspark-gui/static/sse.js",
    "crates/rigspark-gui/static/telemetry.js",
    "site/main.js",
    // Generated, inert catalog data (`cargo catalog-site`); `--check` keeps it reproducible.
    "site/data/latest.js",
    "apps/desktop/src-tauri/src/dialog-smoke.js",
    "crates/rigspark-gui/vendor/dompurify.min.js",
    "crates/rigspark-gui/vendor/katex/contrib/auto-render.min.js",
    "crates/rigspark-gui/vendor/katex/katex.min.js",
    "crates/rigspark-gui/vendor/marked.min.js",
];

// Minified vendors use `node:` as an object key, so only module-loading forms count.
fn imports_node_api(text: &str) -> bool {
    let compact: String = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    ["from", "require(", "import("].iter().any(|loader| {
        ["\"node:", "'node:", "`node:"]
            .iter()
            .any(|quote| compact.contains(&format!("{loader}{quote}")))
    })
}

pub fn check_file(path: &str, text: &str) -> Vec<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let extension = name.rsplit('.').next().unwrap_or("");
    let mut failures = Vec::new();
    if path.starts_with("src/") {
        failures.push("root src directory must be empty after native migration");
    }
    let browser = BROWSER_SCRIPTS.contains(&path);
    if ["ts", "tsx", "jsx", "mjs", "cjs"].contains(&extension) || (extension == "js" && !browser) {
        failures.push("Node/TypeScript source or unreviewed JavaScript remains");
    }
    if [
        "package.json",
        "package-lock.json",
        "npm-shrinkwrap.json",
        "pnpm-lock.yaml",
        "pnpm-workspace.yaml",
        "yarn.lock",
        "bun.lock",
        "bun.lockb",
        ".npmrc",
        ".yarnrc",
        ".yarnrc.yml",
        ".nvmrc",
        ".node-version",
    ]
    .contains(&name)
        || (name.starts_with("tsconfig") && name.ends_with(".json"))
    {
        failures.push("Node package or toolchain configuration remains");
    }
    if browser
        && (imports_node_api(text)
            || text
                .lines()
                .next()
                .is_some_and(|line| line.starts_with("#!") && line.contains("node")))
    {
        failures.push("browser asset invokes a Node runtime or imports Node APIs");
    }
    let executable_config = path.starts_with(".github/workflows/")
        || ["sh", "ps1", "py", "cmd", "bat"].contains(&extension)
        || ["Makefile", "justfile"].contains(&name)
        || path == "apps/desktop/src-tauri/tauri.conf.json";
    if executable_config {
        let lower = text.to_ascii_lowercase();
        if path.starts_with(".github/workflows/")
            && [
                "actions/checkout@",
                "actions/configure-pages@",
                "actions/upload-pages-artifact@",
                "actions/deploy-pages@",
                "actions/add-to-project@",
                "actions/upload-artifact@",
                "actions/download-artifact@",
                "actions/cache@",
                "actions/github-script@",
                "softprops/action-gh-release@",
            ]
            .iter()
            .any(|action| lower.contains(action))
        {
            failures.push("workflow still references a Node-backed action");
        }
        let has_command = lower
            .split(|character: char| {
                !character.is_ascii_alphanumeric() && character != '-' && character != '_'
            })
            .any(|word| {
                [
                    "node",
                    "nodejs",
                    "npm",
                    "npx",
                    "pnpm",
                    "yarn",
                    "bun",
                    "setup-node",
                    "electron",
                ]
                .contains(&word)
            });
        if has_command {
            failures.push("executable configuration still references the Node toolchain");
        }
    }
    failures
}
