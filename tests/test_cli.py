"End-to-end check of the installed binary: rustfmt runs, then compaction joins."
import subprocess, pytest

BROKEN ="""fn time() -> f64 {
    42.0
}
fn pick(c: bool, e: &mut (Option<u8>, Option<u8>)) {
    if c {
        e.0 = Some(1)
    } else {
        e.1 = Some(2)
    }
}
fn notify(ready: bool) {
    if ready {
        drop(1);
    }
}
fn delete_all(fds: Vec<i32>) {
    for fd in fds {
        let _ = fd.checked_add(1);
    }
}
fn keep_two(ready: bool) {
    if ready {
        drop(1);
        drop(2);
    }
}
fn statement_else(c: bool) {
    if c {
        drop(1);
    } else {
        drop(2);
    }
    drop(3);
}
impl F {
    fn wide_assignment() {
        let inner = Arc::new(DapInner { outgoing, pending: pending.clone(), close: close.clone(), closed: closed.clone(), next_seq: AtomicU64::new(1) });
    }
}
"""

def test_fastfmt_binary(tmp_path):
    (tmp_path / 'Cargo.toml').write_text('[package]\nname = "test-fastfmt"\nversion = "0.1.0"\nedition = "2021"\n')
    (tmp_path / 'src').mkdir()
    f = tmp_path / 'src/main.rs'
    f.write_text(BROKEN)
    guard = tmp_path / 'rustfmt.toml'
    r = subprocess.run(['cargo-fastfmt', '--check'], cwd=tmp_path, capture_output=True, text=True)
    assert r.returncode == 1 and not guard.exists()
    assert 'rustfmt.toml' in r.stdout and 'main.rs' in r.stdout
    subprocess.run(['cargo-fastfmt'], cwd=tmp_path, check=True)
    assert guard.read_text() == 'disable_all_formatting = true\n'
    out = f.read_text()
    assert 'fn time() -> f64 { 42.0 }' in out
    assert 'if c { e.0 = Some(1) } else { e.1 = Some(2) }' in out
    assert 'if ready { drop(1); }' in out
    assert 'for fd in fds { let _ = fd.checked_add(1); }' in out
    assert 'drop(1);\n        drop(2);' in out
    assert 'if c { drop(1); }\n    else { drop(2); }' in out
    assert ('        let inner = Arc::new(DapInner { outgoing, pending: pending.clone(), close: close.clone(), '
        'closed: closed.clone(), next_seq: AtomicU64::new(1) });') in out
    assert subprocess.run(['cargo-fastfmt', '--check'], cwd=tmp_path).returncode == 0
    f.write_text(BROKEN)
    subprocess.run(['cargo', 'fmt'], cwd=tmp_path, check=True)
    assert f.read_text() == BROKEN
    r = subprocess.run(['cargo-fastfmt', '--check'], cwd=tmp_path, capture_output=True, text=True)
    assert r.returncode == 1 and 'main.rs' in r.stdout


@pytest.mark.parametrize('edition,source', [
    ('edition.workspace = true', 'fn f(x: Option<u8>) { if let Some(x) = x && x > 0 { drop(x); } }'),
    ('edition = { workspace = true }', 'fn f(x: Option<u8>) { if let Some(x) = x && x > 0 { drop(x); } }'),
    ("edition = '2021' # member override", 'fn gen() {}'),
    ('', 'fn async() {}'),  # Cargo defaults to 2015, not the workspace edition
])
def test_workspace_editions(tmp_path, edition, source):
    (tmp_path/'Cargo.toml').write_text('[workspace]\nmembers = ["py"]\nresolver = "2"\n'
        '[workspace.package]\nedition = "2024"\n')
    member = tmp_path/'py'
    (member/'src').mkdir(parents=True)
    (member/'Cargo.toml').write_text('[package]\nname = "bindings"\nversion = "0.1.0"\n' + edition + '\n')
    (member/'src/lib.rs').write_text(source + '\n')
    subprocess.run(['cargo', 'fastfmt'], cwd=tmp_path, check=True)
    subprocess.run(['cargo', 'fastfmt', '--check'], cwd=tmp_path, check=True)


def test_standalone_file_in_virtual_workspace(tmp_path):
    (tmp_path/'Cargo.toml').write_text('[workspace]\nmembers = ["member"]\nresolver = "2"\n')
    src = tmp_path/'member/src'
    src.mkdir(parents=True)
    (src.parent/'Cargo.toml').write_text('[package]\nname = "member"\nversion = "0.1.0"\nedition = "2021"\n')
    (src/'lib.rs').write_text('fn member() {}\n')
    script = tmp_path/'script.rs'
    script.write_text('async fn main() {\n    work().await;\n}\n')
    subprocess.run(['cargo-fastfmt'], cwd=tmp_path, check=True)
    assert script.read_text() == 'async fn main() { work().await; }\n'
    subprocess.run(['cargo-fastfmt', '--check'], cwd=tmp_path, check=True)
