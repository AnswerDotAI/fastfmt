"End-to-end check of the installed binary: rustfmt runs, then compaction joins."
import subprocess

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
"""

def test_fastfmt_binary(tmp_path):
    (tmp_path / 'Cargo.toml').write_text('[package]\nedition = "2021"\n')
    f = tmp_path / 'main.rs'
    f.write_text(BROKEN)
    subprocess.run(['cargo-fastfmt'], cwd=tmp_path, check=True)
    out = f.read_text()
    assert 'fn time() -> f64 { 42.0 }' in out
    assert 'if c { e.0 = Some(1) } else { e.1 = Some(2) }' in out
    assert subprocess.run(['cargo-fastfmt', '--check'], cwd=tmp_path).returncode == 0
    f.write_text(BROKEN)
    r = subprocess.run(['cargo-fastfmt', '--check'], cwd=tmp_path, capture_output=True, text=True)
    assert r.returncode == 1 and 'main.rs' in r.stdout
