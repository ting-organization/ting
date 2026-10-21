use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
use ting_node::wiki;

fn attribute_values<'a>(html: &'a str, name: &str) -> Vec<&'a str> {
    let mut values = Vec::new();
    for quote in ['"', '\''] {
        let marker = format!("{name}={quote}");
        let mut rest = html;
        while let Some(start) = rest.find(&marker) {
            rest = &rest[start + marker.len()..];
            let end = rest.find(quote).expect("closed attribute");
            values.push(&rest[..end]);
            rest = &rest[end..];
        }
    }
    values
}
fn markdown_files(dir: &Path, out: &mut BTreeSet<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            markdown_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let relative = path.strip_prefix(&root).unwrap();
            out.insert(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

#[test]
fn every_page_renders_with_exactly_one_title() {
    for page in wiki::all() {
        let html = wiki::page(page.path).unwrap();
        assert_eq!(html.matches("<h1").count(), 1, "{}", page.url());
        assert!(html.contains("aria-current='page'"), "{}", page.url());
    }
}

#[test]
fn registry_and_wiki_files_match() {
    let mut files = BTreeSet::new();
    markdown_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("wiki"),
        &mut files,
    );
    let registered: BTreeSet<_> = wiki::all()
        .map(|p| p.file.to_string())
        .filter(|f| f.starts_with("wiki/"))
        .collect();
    assert_eq!(files, registered);
    let paths: BTreeSet<_> = wiki::all().map(|p| p.path).collect();
    assert_eq!(paths.len(), wiki::all().count(), "duplicate page path");
}

#[test]
fn internal_links_and_anchors_resolve() {
    let site = ["/", "/docs", "/openapi.json"];
    for page in wiki::all() {
        let html = wiki::page(page.path).unwrap();
        let ids: BTreeSet<_> = attribute_values(&html, "id").into_iter().collect();
        for href in attribute_values(&html, "href") {
            let href = href.replace("&amp;", "&");
            assert!(
                !href.contains(".md"),
                "{}: unrewritten link {href}",
                page.url()
            );
            if href.starts_with("http") || href.starts_with("/assets/") || site.contains(&&*href) {
                continue;
            }
            let (target, fragment) = href
                .split_once('#')
                .map_or((&*href, None), |(t, f)| (t, Some(f)));
            if target.is_empty() {
                assert!(
                    ids.contains(fragment.unwrap()),
                    "{}: missing anchor #{}",
                    page.url(),
                    fragment.unwrap()
                );
                continue;
            }
            let path = target
                .strip_prefix("/wiki")
                .unwrap_or_else(|| panic!("{}: unexpected link {href}", page.url()))
                .trim_start_matches('/');
            if wiki::file(path).is_some() {
                assert!(fragment.is_none());
                continue;
            }
            let linked =
                wiki::page(path).unwrap_or_else(|| panic!("{}: broken link {href}", page.url()));
            if let Some(fragment) = fragment {
                assert!(
                    attribute_values(&linked, "id").contains(&fragment),
                    "{}: missing anchor {href}",
                    page.url()
                );
            }
        }
    }
}

#[test]
fn trailing_slashes_redirect() {
    for (slashed, target) in [
        ("consensus/", "/wiki/consensus"),
        ("start/node/", "/wiki/start/node"),
    ] {
        assert!(wiki::page(slashed).is_none());
        assert_eq!(wiki::redirect(slashed).as_deref(), Some(target));
    }
    assert!(wiki::redirect("missing").is_none());
    assert!(wiki::redirect("missing/").is_none());
    assert!(wiki::page("../Cargo.toml").is_none());
}

#[test]
fn every_reference_program_is_served_byte_for_byte() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../programs");
    let mut served = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        let (mime, body) = match name.rsplit_once('.') {
            Some((_, "wasm" | "json")) => wiki::file(&format!("programs/{name}"))
                .unwrap_or_else(|| panic!("{name} is not served")),
            _ => continue,
        };
        assert_eq!(body, std::fs::read(dir.join(&name)).unwrap(), "{name}");
        let expected = if name.ends_with(".wasm") {
            "application/wasm"
        } else {
            "application/json; charset=utf-8"
        };
        assert_eq!(mime, expected, "{name}");
        served += 1;
    }
    assert_eq!(served, 8, "seven programs and their manifest");
    assert!(wiki::file("programs/README.md").is_none());
    assert!(wiki::file("programs/../Cargo.toml").is_none());
}
#[test]
fn diagrams_are_drawn_into_their_pages() {
    let mut drawn = 0;
    for page in wiki::all() {
        let html = wiki::page(page.path).unwrap();
        assert!(
            !html.contains("diagrams/"),
            "{}: a diagram left as a link",
            page.url()
        );
        drawn += html.matches("<figure class='diagram'><svg").count();
    }
    assert!(drawn >= 14, "{drawn} diagrams drawn");
}
#[test]
fn documents_link_each_other_by_relative_paths() {
    // A relative link works where the file is read, in the repository or on
    // GitHub, and the wiki turns it into its own URL; a /wiki link works only
    // in the wiki.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for page in wiki::all() {
        let text = std::fs::read_to_string(root.join(page.file)).unwrap();
        for absolute in ["](/wiki", "href=\"/wiki"] {
            assert!(
                !text.contains(absolute),
                "{}: a /wiki link; link to the page's file by a relative path",
                page.file
            );
        }
    }
}
