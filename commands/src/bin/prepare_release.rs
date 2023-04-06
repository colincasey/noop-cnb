use clap::{Parser, ValueEnum};
use console::Emoji;
use glob::glob;
use semver::{BuildMetadata, Prerelease, Version};
use std::collections::{HashMap, HashSet};
use std::fmt::format;
use std::path::{Path, PathBuf};
use std::{env, fs};
use tree_sitter::{Node, Parser as TreeSitterParser, Query, QueryCursor, Range};

static LOOKING_GLASS: Emoji<'_, '_> = Emoji("🔍 ", "");
static CROSS_MARK: Emoji<'_, '_> = Emoji("❌ ", "");
static WARNING: Emoji<'_, '_> = Emoji("⚠️ ", "");
static CHECK: Emoji<'_, '_> = Emoji("✅️ ", "");

const UNSPECIFIED_ERROR: i32 = 1;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long, value_enum)]
    bump: VersionCoordinate,
}

#[derive(ValueEnum, Debug, Clone)]
enum VersionCoordinate {
    Major,
    Minor,
    Patch,
}

#[derive(Eq, PartialEq, Hash)]
struct TargetToPrepare {
    path: PathBuf,
    buildpack_toml: BuildpackToml,
    changelog_md: ChangelogMarkdown,
}

#[derive(Eq, PartialEq, Hash)]
struct BuildpackToml {
    path: PathBuf,
    contents: String,
    current_version: Version,
    current_version_location: Range,
}

#[derive(Eq, PartialEq, Hash)]
struct ChangelogMarkdown {
    path: PathBuf,
    contents: String,
}

fn main() {
    let args = Args::parse();
    let targets_to_prepare = find_directories_containing_a_buildpack_and_changelog();

    let current_version = get_fixed_version(&targets_to_prepare);
    // println!("{current_version}");

    let next_version = calculate_next_version(current_version, args.bump);
    // println!("{next_version}");

    for target_to_prepare in targets_to_prepare {
        update_buildpack_version_and_changelog(target_to_prepare, &next_version);
    }
}

fn find_directories_containing_a_buildpack_and_changelog() -> Vec<TargetToPrepare> {
    eprintln!("{LOOKING_GLASS} Looking for Buildpacks & Changelogs");
    let current_dir = get_current_dir();

    let buildpack_dirs: HashSet<_> =
        match glob(&current_dir.join("**/buildpack.toml").to_string_lossy()) {
            Ok(paths) => paths
                .filter_map(Result::ok)
                .map(|path| parent_dir(&path))
                .collect(),
            Err(error) => {
                fail_with_error(format!(
                    "Failed to glob buildpack.toml files in {}: {}",
                    current_dir.to_string_lossy(),
                    error
                ));
            }
        };

    let changelog_dirs: HashSet<_> =
        match glob(&current_dir.join("**/CHANGELOG.md").to_string_lossy()) {
            Ok(paths) => paths
                .filter_map(Result::ok)
                .map(|path| parent_dir(&path))
                .collect(),
            Err(error) => {
                fail_with_error(format!(
                    "Failed to glob CHANGELOG.md files in {}: {}",
                    current_dir.to_string_lossy(),
                    error
                ));
            }
        };

    let (dirs_with_a_changelog_and_buildpack, dirs_without): (HashSet<_>, HashSet<_>) =
        buildpack_dirs
            .into_iter()
            .partition(|dir| changelog_dirs.contains(dir));

    for dir_without in dirs_without {
        eprintln!(
            "{WARNING} Ignoring {}: buildpack.toml found but no CHANGELOG.md",
            dir_without.to_string_lossy()
        );
    }

    dirs_with_a_changelog_and_buildpack
        .iter()
        .map(|dir| create_target_to_prepare(dir))
        .collect()
}

fn create_target_to_prepare(dir: &Path) -> TargetToPrepare {
    let buildpack_toml_path = dir.join("buildpack.toml");
    let buildpack_toml_contents = match fs::read_to_string(&buildpack_toml_path) {
        Ok(contents) => contents,
        Err(error) => fail_with_error(format!(
            "Could not read contents of {}: {}",
            buildpack_toml_path.to_string_lossy(),
            error
        )),
    };
    let (version, range) =
        extract_version_from_buildpack_toml(&buildpack_toml_path, &buildpack_toml_contents);

    let buildpack_toml = BuildpackToml {
        path: buildpack_toml_path,
        contents: buildpack_toml_contents,
        current_version: version,
        current_version_location: range,
    };

    let changelog_md_path = dir.join("CHANGELOG.md");
    let changelog_md_contents = match fs::read_to_string(&changelog_md_path) {
        Ok(contents) => contents,
        Err(error) => fail_with_error(format!(
            "Could not read contents of {}: {}",
            changelog_md_path.to_string_lossy(),
            error
        )),
    };
    let changelog_md = ChangelogMarkdown {
        path: changelog_md_path,
        contents: changelog_md_contents,
    };

    TargetToPrepare {
        path: dir.to_path_buf(),
        buildpack_toml,
        changelog_md,
    }
}

// why not just use toml_edit?
// because it doesn't retain the ordering of toml keys and will rewrite the document entirely :(
// but using an AST lets us identify the exact line that needs to be updated
fn extract_version_from_buildpack_toml(path: &Path, contents: &String) -> (Version, Range) {
    let mut parser = TreeSitterParser::new();
    parser
        .set_language(tree_sitter_toml::language())
        .expect("Treesitter TOML grammar should load");

    let tree = match parser.parse(contents, None) {
        Some(tree) => tree,
        None => fail_with_error(format!("Could not parse {}", path.to_string_lossy())),
    };

    // captures the version entry in the toml document that looks like:
    // [buildpack]         # table
    // version = "x.y.z"   # pair
    let query_results = query_toml_ast(
        r#"
            (
              (document
                (table
                  (bare_key) @table-name
                  (pair
                    (bare_key) @property-name
                    (string) @version
                  )
                )
              )
              (#eq? @table-name "buildpack")
              (#eq? @property-name "version")
            )
        "#,
        tree.root_node(),
        contents.as_bytes(),
    );

    match query_results.get("version") {
        Some(result) => {
            let range = result.range();
            // toml strings are quoted so we want to remove those to get the inner value
            let value = String::from(&contents[range.start_byte + 1..range.end_byte - 1]);
            let version = match Version::parse(&value) {
                Ok(parsed_version) => parsed_version,
                Err(error) => fail_with_error(format!(
                    "Version {} from {} is invalid: {}",
                    value,
                    path.to_string_lossy(),
                    error
                )),
            };
            (version, range)
        }
        None => fail_with_error(format!("No version found in {}", path.to_string_lossy())),
    }
}

fn get_fixed_version(targets_to_prepare: &[TargetToPrepare]) -> Version {
    let all_versions: HashSet<_> = targets_to_prepare
        .iter()
        .map(|target_to_prepare| target_to_prepare.buildpack_toml.current_version.clone())
        .collect();

    if all_versions.len() != 1 {
        fail_with_error(format!(
            "Not all versions match:\n{}",
            targets_to_prepare
                .iter()
                .map(|target_to_prepare| {
                    format!(
                        "• {} ({})",
                        target_to_prepare.buildpack_toml.path.to_string_lossy(),
                        target_to_prepare.buildpack_toml.current_version
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    let target_to_prepare = targets_to_prepare
        .first()
        .expect("There should only be one");

    target_to_prepare.buildpack_toml.current_version.clone()
}

fn calculate_next_version(current_version: Version, coordinate: VersionCoordinate) -> Version {
    let Version {
        major,
        minor,
        patch,
        ..
    } = current_version;

    match coordinate {
        VersionCoordinate::Major => Version {
            major: major + 1,
            minor: 0,
            patch: 0,
            pre: Prerelease::EMPTY,
            build: BuildMetadata::EMPTY,
        },
        VersionCoordinate::Minor => Version {
            major,
            minor: minor + 1,
            patch: 0,
            pre: Prerelease::EMPTY,
            build: BuildMetadata::EMPTY,
        },
        VersionCoordinate::Patch => Version {
            major,
            minor,
            patch: patch + 1,
            pre: Prerelease::EMPTY,
            build: BuildMetadata::EMPTY,
        },
    }
}

fn update_buildpack_version_and_changelog(target_to_prepare: TargetToPrepare, version: &Version) {
    update_buildpack_toml(&target_to_prepare.buildpack_toml, version);
    update_changelog_md(&target_to_prepare.changelog_md, version);
}

fn update_buildpack_toml(buildpack_toml: &BuildpackToml, version: &Version) {
    eprintln!(
        "{CHECK} Updating version {} → {}: {}",
        buildpack_toml.current_version,
        version,
        buildpack_toml.path.to_string_lossy(),
    );

    let new_contents = format!(
        "{}\"{}\"{}",
        &buildpack_toml.contents[..buildpack_toml.current_version_location.start_byte],
        version,
        &buildpack_toml.contents[buildpack_toml.current_version_location.end_byte..]
    );

    if let Err(error) = fs::write(&buildpack_toml.path, new_contents) {
        fail_with_error(format!(
            "Could not write to {}: {error}",
            &buildpack_toml.path.to_string_lossy()
        ));
    }
}

fn update_changelog_md(_changelog_md: &ChangelogMarkdown, _version: &Version) {
    //todo!()
}

fn get_current_dir() -> PathBuf {
    match env::current_dir() {
        Ok(current_dir) => current_dir,
        Err(io_error) => {
            fail_with_error(format!("Could not determine current directory: {io_error}"));
        }
    }
}

fn parent_dir(path: &Path) -> PathBuf {
    if let Some(parent) = path.parent() {
        parent.to_path_buf()
    } else {
        fail_with_error(format!(
            "Could not get parent directory from {}",
            path.to_string_lossy()
        ));
    }
}

fn fail_with_error<IntoString: Into<String>>(error: IntoString) -> ! {
    eprintln!("{CROSS_MARK} {}", error.into());
    std::process::exit(UNSPECIFIED_ERROR);
}

fn query_toml_ast<'a>(query: &str, node: Node<'a>, source: &[u8]) -> HashMap<String, Node<'a>> {
    let query = Query::new(tree_sitter_toml::language(), &query)
        .expect(format!("TOML AST query is invalid: {}", query).as_str());
    query_ast(query, node, source)
}

fn query_ast<'a>(query: Query, node: Node<'a>, source: &[u8]) -> HashMap<String, Node<'a>> {
    let mut query_results: HashMap<String, Node> = HashMap::new();
    let mut query_cursor = QueryCursor::new();
    let capture_names = query.capture_names();
    let query_matches = query_cursor.matches(&query, node, source);
    for query_match in query_matches {
        for capture in query_match.captures {
            let capture_name = &capture_names[capture.index as usize];
            query_results.insert(capture_name.clone(), capture.node.clone());
        }
    }
    query_results
}
