//! `python/chroma_memory.py`, run for real under `python3` against a stand-in for the
//! `chromadb` package (`fixtures/fake_chroma`). Real ChromaDB and the embedding model
//! are not needed or tested; what is checked is the helper's own logic: that it needs
//! nothing but a database path, what it upserts and in which batches, which filters it
//! builds, and what it prints. They need `python3`; without it each test prints SKIPPED.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};
use tempfile::TempDir;

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn helper() -> PathBuf {
    manifest().join("../../python/chroma_memory.py")
}

fn have_python() -> bool {
    Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

macro_rules! require_python {
    () => {
        if !have_python() {
            eprintln!("SKIPPED: python3 not found");
            return;
        }
    };
}

struct Run {
    out: String,
    err: String,
    code: i32,
}

/// Run the helper from an unrelated folder, with a clean environment. `fake` puts the
/// stand-in chromadb on the path.
fn run(cwd: &Path, fake: bool, env: &[(&str, &str)], args: &[&str]) -> Run {
    let mut cmd = Command::new("python3");
    cmd.arg(helper()).args(args).current_dir(cwd).env_clear();
    cmd.env("PATH", std::env::var_os("PATH").unwrap_or_default());
    cmd.env("HOME", cwd);
    if fake {
        cmd.env("PYTHONPATH", manifest().join("tests/fixtures/fake_chroma"));
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let o: Output = cmd.output().expect("run python3");
    Run {
        out: String::from_utf8_lossy(&o.stdout).into_owned(),
        err: String::from_utf8_lossy(&o.stderr).into_owned(),
        code: o.status.code().unwrap_or(-1),
    }
}

fn store(db: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(db.join("store.json")).unwrap()).unwrap()
}

/// `n` chunk lines for sessions 1..=n, shaped like the chunker's.
fn chunks(n: usize) -> String {
    (0..n)
        .map(|i| {
            json!({
                "id": format!("s{:02}_c{i:03}", i % 7 + 1),
                "document": format!("Mara_Quill: the lantern number {i} is lit at the docks."),
                "metadata": {
                    "session": i % 7 + 1,
                    "start": "00:01:00",
                    "end": "00:02:00",
                    "characters": "Mara_Quill, GM",
                    "has_Mara_Quill": true,
                },
            })
            .to_string()
                + "\n"
        })
        .collect()
}

#[test]
fn ingest_upserts_in_batches_of_100_and_says_how_many() {
    require_python!();
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("store");
    let jsonl = tmp.path().join("chunks.jsonl");
    fs::write(&jsonl, chunks(250) + "\n   \n").unwrap(); // blank lines are skipped
    let r = run(
        tmp.path(),
        true,
        &[],
        &["ingest", "--db", db.to_str().unwrap(), jsonl.to_str().unwrap()],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        r.out.trim(),
        format!("Upserted 250 chunks into {}. Collection now holds 250.", db.display())
    );
    let s = store(&db);
    assert_eq!(s["batches"], json!([100, 100, 50]));
    assert_eq!(s["name"], "transcripts");
    assert_eq!(s["model"], "BAAI/bge-base-en-v1.5");
    assert_eq!(
        s["configuration"],
        json!({"hnsw": {"space": "cosine"}}),
        "cosine distance"
    );

    // Running it again overwrites instead of duplicating.
    let r = run(
        tmp.path(),
        true,
        &[],
        &["ingest", "--db", db.to_str().unwrap(), jsonl.to_str().unwrap()],
    );
    assert!(
        r.out.contains("Upserted 250 chunks") && r.out.contains("Collection now holds 250."),
        "{}",
        r.out
    );
}

#[test]
fn the_embedding_model_can_be_changed_by_the_environment() {
    require_python!();
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("store");
    let jsonl = tmp.path().join("c.jsonl");
    fs::write(&jsonl, chunks(1)).unwrap();
    let r = run(
        tmp.path(),
        true,
        &[("TTRPG_EMBED", "all-MiniLM-L6-v2")],
        &["ingest", "--db", db.to_str().unwrap(), jsonl.to_str().unwrap()],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(store(&db)["model"], "all-MiniLM-L6-v2");
}

#[test]
fn the_database_comes_from_db_or_from_the_environment_and_db_wins() {
    require_python!();
    let tmp = TempDir::new().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    let jsonl = tmp.path().join("c.jsonl");
    fs::write(&jsonl, chunks(1)).unwrap();
    let j = jsonl.to_str().unwrap();

    let r = run(tmp.path(), true, &[("TTRPG_DB", a.to_str().unwrap())], &["ingest", j]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(a.join("store.json").is_file());

    let r = run(
        tmp.path(),
        true,
        &[("TTRPG_DB", a.to_str().unwrap())],
        &["ingest", "--db", b.to_str().unwrap(), j],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(b.join("store.json").is_file(), "--db beats $TTRPG_DB");
}

#[test]
fn without_a_database_it_says_so_and_does_not_need_chromadb_to_say_it() {
    require_python!();
    let tmp = TempDir::new().unwrap();
    let r = run(tmp.path(), false, &[], &["info"]);
    assert_eq!(r.code, 2);
    assert!(r.err.contains("give --db, or set $TTRPG_DB"), "{}", r.err);

    let r = run(
        tmp.path(),
        false,
        &[],
        &[
            "ingest",
            "--db",
            tmp.path().join("s").to_str().unwrap(),
            "/no/such.jsonl",
        ],
    );
    assert_eq!(r.code, 2);
    assert!(r.err.contains("chunk file not found: /no/such.jsonl"), "{}", r.err);
    assert!(!tmp.path().join("s").exists(), "nothing was created");
}

#[test]
fn info_on_a_missing_store_needs_no_chromadb() {
    require_python!();
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("nothing-here");
    let r = run(
        tmp.path(),
        false,
        &[],
        &["info", "--db", db.to_str().unwrap(), "--pcs", "Mara Quill, Tomas Ink"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    for want in [
        format!("db path    : {}", db.display()),
        "exists     : False".to_string(),
        "embeddings : BAAI/bge-base-en-v1.5".to_string(),
        "pcs        : Mara Quill, Tomas Ink".to_string(),
        "chunks     : 0 (no store yet)".to_string(),
    ] {
        assert!(r.out.contains(&want), "missing {want:?} in:\n{}", r.out);
    }
}

#[test]
fn info_on_a_store_counts_chunks_and_lists_sessions() {
    require_python!();
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("store");
    let jsonl = tmp.path().join("c.jsonl");
    fs::write(&jsonl, chunks(30)).unwrap();
    run(
        tmp.path(),
        true,
        &[],
        &["ingest", "--db", db.to_str().unwrap(), jsonl.to_str().unwrap()],
    );
    let r = run(tmp.path(), true, &[], &["info", "--db", db.to_str().unwrap()]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.contains("exists     : True"), "{}", r.out);
    assert!(r.out.contains("pcs        : (none)"), "{}", r.out);
    assert!(r.out.contains("chunks     : 30"), "{}", r.out);
    assert!(r.out.contains("sessions   : 1, 2, 3, 4, 5, 6, 7"), "{}", r.out);
}

#[test]
fn query_builds_its_filters_and_prints_what_it_found() {
    require_python!();
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("store");
    let jsonl = tmp.path().join("c.jsonl");
    fs::write(&jsonl, chunks(30)).unwrap();
    let d = db.to_str().unwrap();
    run(tmp.path(), true, &[], &["ingest", "--db", d, jsonl.to_str().unwrap()]);

    // Plain: no filters, six results.
    let r = run(tmp.path(), true, &[], &["query", "--db", d, "lantern number 4"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        store(&db)["last_query"],
        json!({"n": 6, "where": null, "where_document": null})
    );
    assert_eq!(r.out.matches("=== s").count(), 6, "{}", r.out);
    let header = r.out.lines().find(|l| l.starts_with("=== s")).unwrap();
    assert!(
        header.starts_with("=== s") && header.contains(" 00:01:00-00:02:00 [Mara_Quill, GM] cos_dist="),
        "{header}"
    );
    assert!(header.ends_with(" ==="), "{header}");

    // A session floor alone is a bare condition.
    run(
        tmp.path(),
        true,
        &[],
        &["query", "--db", d, "lantern", "--from", "5", "-k", "3"],
    );
    assert_eq!(
        store(&db)["last_query"],
        json!({"n": 3, "where": {"session": {"$gte": 5}}, "where_document": null})
    );

    // A character with a space is looked up the way the chunker stores it.
    run(
        tmp.path(),
        true,
        &[],
        &["query", "--db", d, "lantern", "--char", "Mara Quill"],
    );
    assert_eq!(store(&db)["last_query"]["where"], json!({"has_Mara_Quill": true}));

    // Several conditions are all required; the text filter goes in its own slot.
    let r = run(
        tmp.path(),
        true,
        &[],
        &[
            "query",
            "--db",
            d,
            "lantern",
            "--from",
            "2",
            "--char",
            "Mara Quill",
            "--contains",
            "number 29",
        ],
    );
    assert_eq!(
        store(&db)["last_query"],
        json!({
            "n": 6,
            "where": {"$and": [{"session": {"$gte": 2}}, {"has_Mara_Quill": true}]},
            "where_document": {"$contains": "number 29"},
        })
    );
    assert_eq!(r.out.matches("=== s").count(), 1, "{}", r.out);

    // Nothing matches: it says so, with the path.
    let r = run(
        tmp.path(),
        true,
        &[],
        &["query", "--db", d, "lantern", "--contains", "dragon"],
    );
    assert_eq!(r.out.trim(), format!("No results in {}.", db.display()));
}

#[test]
fn a_long_chunk_is_cut_at_600_characters() {
    require_python!();
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("store");
    let jsonl = tmp.path().join("c.jsonl");
    let long = "word ".repeat(300);
    fs::write(
        &jsonl,
        json!({"id": "x", "document": long, "metadata": {"session": 1, "start": "a", "end": "b", "characters": "c"}})
            .to_string()
            + "\n",
    )
    .unwrap();
    let d = db.to_str().unwrap();
    run(tmp.path(), true, &[], &["ingest", "--db", d, jsonl.to_str().unwrap()]);
    let r = run(tmp.path(), true, &[], &["query", "--db", d, "word"]);
    let body = r.out.lines().skip_while(|l| !l.starts_with("=== s")).nth(1).unwrap();
    assert_eq!(body.chars().count(), 600);
}

#[test]
fn a_missing_chromadb_is_a_python_import_error_the_program_can_recognise() {
    require_python!();
    let tmp = TempDir::new().unwrap();
    let jsonl = tmp.path().join("c.jsonl");
    fs::write(&jsonl, chunks(1)).unwrap();
    let r = run(
        tmp.path(),
        false,
        &[],
        &[
            "ingest",
            "--db",
            tmp.path().join("s").to_str().unwrap(),
            jsonl.to_str().unwrap(),
        ],
    );
    // Only when this machine has no chromadb; if it does, the helper simply works.
    if r.code != 0 {
        assert!(r.err.contains("No module named 'chromadb'"), "{}", r.err);
    }
}
