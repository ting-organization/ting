#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Explorer,
    Wiki,
    Api,
}

pub fn page(
    title: &str,
    section: Section,
    selected: &str,
    body: &str,
    head: &str,
    scripts: &str,
) -> String {
    let navigation = match section {
        Section::Wiki => crate::wiki::navigation(selected),
        Section::Explorer => include_str!("../web/explorer-nav.html").to_string(),
        Section::Api => String::new(),
    };
    let active = |item| {
        if section == item {
            "aria-current='page'"
        } else {
            ""
        }
    };
    include_str!("../web/shell.html")
        .replace("{{title}}", title)
        .replace(
            "{{section}}",
            match section {
                Section::Explorer => "explorer",
                Section::Wiki => "wiki",
                Section::Api => "api",
            },
        )
        .replace("{{node_active}}", active(Section::Explorer))
        .replace("{{wiki_active}}", active(Section::Wiki))
        .replace("{{api_active}}", active(Section::Api))
        .replace("{{navigation}}", &navigation)
        .replace("{{body}}", body)
        .replace("{{head}}", head)
        .replace("{{scripts}}", scripts)
        .replace("{{version}}", env!("CARGO_PKG_VERSION"))
}
pub fn explorer() -> String {
    page(
        "Explorer",
        Section::Explorer,
        "",
        include_str!("../web/explorer.html"),
        "",
        "<script src='/assets/explorer.js' defer></script>",
    )
}
pub fn api_docs() -> String {
    page(
        "API reference",
        Section::Api,
        "",
        include_str!("../web/api.html"),
        "<link rel='stylesheet' href='/assets/swagger-ui.css'>",
        "<script src='/assets/swagger-ui-bundle.js' defer></script><script src='/assets/swagger-init.js' defer></script>",
    )
}
pub fn asset(name: &str) -> Option<(&'static str, &'static str)> {
    let (mime, body) = match name {
        "site.css" => ("text/css; charset=utf-8", include_str!("../web/site.css")),
        "logo.svg" => ("image/svg+xml", include_str!("../web/logo.svg")),
        "site.js" => (
            "application/javascript; charset=utf-8",
            include_str!("../web/site.js"),
        ),
        "explorer.js" => (
            "application/javascript; charset=utf-8",
            include_str!("../web/explorer.js"),
        ),
        "swagger-init.js" => (
            "application/javascript; charset=utf-8",
            include_str!("../web/swagger-init.js"),
        ),
        "swagger-ui.css" => (
            "text/css; charset=utf-8",
            include_str!("../vendor/swagger-ui/swagger-ui.css"),
        ),
        "swagger-ui-bundle.js" => (
            "application/javascript; charset=utf-8",
            include_str!("../vendor/swagger-ui/swagger-ui-bundle.js"),
        ),
        "swagger-ui-license.txt" => (
            "text/plain; charset=utf-8",
            include_str!("../vendor/swagger-ui/LICENSE"),
        ),
        "swagger-ui-notice.txt" => (
            "text/plain; charset=utf-8",
            include_str!("../vendor/swagger-ui/NOTICE"),
        ),
        _ => return None,
    };
    Some((mime, body))
}
