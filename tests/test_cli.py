"End-to-end checks of the installed binary: rustfmt runs, then compaction joins."
import json, subprocess, pytest

def package(path, src, edition='edition = "2021"', extra='', name='member'):
    "Write a package at `path` whose `src/lib.rs` holds `src`, and return that file."
    (path/'src').mkdir(parents=True, exist_ok=True)
    (path/'Cargo.toml').write_text(f'[package]\nname = "{name}"\nversion = "0.1.0"\n{edition}\n{extra}\n')
    lib = path/'src/lib.rs'
    lib.write_text(src)
    return lib

def workspace(path, members, extra=''):
    "Write a virtual workspace manifest at `path` listing `members`."
    (path/'Cargo.toml').write_text(f'[workspace]\nmembers = {json.dumps(members)}\nresolver = "2"\n{extra}\n')

def fastfmt(cwd, *args): return subprocess.run(['cargo-fastfmt', *args], cwd=cwd, capture_output=True, text=True)

def formats_clean(cwd, *args):
    "Format in `cwd`, then confirm `--check` finds nothing left to change."
    r = fastfmt(cwd, *args)
    assert r.returncode == 0, r.stderr
    assert fastfmt(cwd, *args, '--check').returncode == 0


# Joined, this is 120 characters: wider than rustfmt's default of 100, and within fastfmt's default of 160.
SOURCE = 'fn wide() {\n    let inner = Arc::new(DapInner { pending: pending.clone(), close: close.clone(), closed: closed.clone() });\n}\n'

def test_fastfmt_binary(tmp_path):
    f = package(tmp_path, SOURCE)
    guard = tmp_path/'rustfmt.toml'
    r = fastfmt(tmp_path, '--check')
    assert r.returncode == 1 and not guard.exists()
    assert 'rustfmt.toml' in r.stdout and 'lib.rs' in r.stdout
    formats_clean(tmp_path)
    assert guard.read_text() == 'disable_all_formatting = true\n'
    out = f.read_text()
    assert out == 'fn wide() { let inner = Arc::new(DapInner { pending: pending.clone(), close: close.clone(), closed: closed.clone() }); }\n'
    subprocess.run(['cargo', 'fmt'], cwd=tmp_path, check=True)
    assert f.read_text() == out
    f.write_text(SOURCE)
    r = fastfmt(tmp_path, '--check')
    assert r.returncode == 1 and 'lib.rs' in r.stdout and 'rustfmt.toml' not in r.stdout


def test_ignored_targets(tmp_path):
    subprocess.run(['git', 'init', '-q', str(tmp_path)], check=True)
    source = 'fn f() {\n    work();\n}\n'
    lib = package(tmp_path, source)
    (tmp_path/'.gitignore').write_text('links/\nignored.rs\nrustfmt.toml\nsrc/*.rs\n')
    (tmp_path/'src/.gitignore').write_text('!lib.rs\n')
    (tmp_path/'links/src').mkdir(parents=True)
    subprocess.run(['git', 'init', '-q', str(tmp_path/'links/src')], check=True)
    (tmp_path/'links/Cargo.toml').write_text('invalid Cargo manifest\n')
    untouched = ['links/src/lib.rs', 'ignored.rs', 'src/generated.rs', '.hidden/lib.rs', 'target/lib.rs']
    for name in untouched:
        (tmp_path/name).parent.mkdir(parents=True, exist_ok=True)
        (tmp_path/name).write_text(source)
    def snapshot(): return {f: f.read_bytes() for f in tmp_path.rglob('*') if f.is_file()}
    targets = [('.', False), ('src', False), ('src/lib.rs', True), ('links', False), ('links/src', True), ('links/src/lib.rs', False),
        ('ignored.rs', True), ('src/generated.rs', False)]
    for target, absolute in targets:
        lib.write_text(source)
        arg = str(tmp_path/target) if absolute else target
        formats = target in ('.', 'src', 'src/lib.rs')
        before = snapshot()
        assert fastfmt(tmp_path, arg, '--check').returncode == int(formats), target
        assert snapshot() == before, target
        formats_clean(tmp_path, arg)
        assert lib.read_text() == ('fn f() { work(); }\n' if formats else source), target
        assert all((tmp_path/name).read_text() == source for name in untouched), target
        assert not (tmp_path/'rustfmt.toml').exists() and not (tmp_path/'links/rustfmt.toml').exists(), target


def test_workspace_editions(tmp_path):
    let_chain = 'fn f(x: Option<u8>) { if let Some(x) = x && x > 0 { drop(x); } }\n'
    members = {
        'dotted': ('edition.workspace = true', let_chain),
        'inline': ('edition = { workspace = true }', let_chain),
        'pinned': ("edition = '2021' # member override", 'fn gen() {}\n'),
        'unset': ('', 'fn async() {}\n'),  # Cargo defaults to 2015, not the workspace edition
    }
    workspace(tmp_path, list(members), '[workspace.package]\nedition = "2024"')
    for name, (edition, source) in members.items(): package(tmp_path/name, source, edition, name=name)
    # The only run through Cargo's subcommand dispatch
    subprocess.run(['cargo', 'fastfmt'], cwd=tmp_path, check=True)
    subprocess.run(['cargo', 'fastfmt', '--check'], cwd=tmp_path, check=True)


def test_virtual_workspace(tmp_path):
    subprocess.run(['git', 'init', '-q', str(tmp_path)], check=True)
    workspace(tmp_path, ['member'])
    (tmp_path/'rustfmt.toml').write_text('max_width = 120\n')
    package(tmp_path/'member', 'fn member() {}\n')
    script = tmp_path/'script.rs'
    script.write_text('async fn main() {\n    work().await;\n}\n')
    formats_clean(tmp_path)
    assert script.read_text() == 'async fn main() { work().await; }\n'
    # rustfmt reads the root config for the member as well.
    assert (tmp_path/'rustfmt.toml').read_text() == 'max_width = 120\ndisable_all_formatting = true\n'
    assert not (tmp_path/'member/rustfmt.toml').exists()


@pytest.mark.parametrize('ws_setting,pkg_setting,enabled', [
    ('', '', False),
    ('', 'join_comments = true', True),
    ('join_comments = true', '', True),
    ('join_comments = true', 'join_comments = false', False),
    ('', 'join_comments = "yes"', None),
])
def test_comment_settings(tmp_path, ws_setting, pkg_setting, enabled):
    workspace(tmp_path, ['member'], '[workspace.metadata.fastfmt]\n' + ws_setting)
    source = ('/// A wrapped paragraph\n/// continues here.\n///\n/// - First item\n/// - Second item\n'
        '///\n/// ```rust\n/// let x = 1;\n/// let y = 2;\n/// ```\n///\n/// Heading\n/// =======\n'
        '///\n/// First line  \n/// second line\\\n/// third line\nfn f() {}\n')
    f = package(tmp_path/'member', source, extra='[package.metadata.fastfmt]\n' + pkg_setting)
    if enabled is None:
        r = fastfmt(tmp_path)
        assert r.returncode == 2 and 'join_comments must be a boolean' in r.stderr
        assert f.read_text() == source
        return
    formats_clean(tmp_path)
    assert f.read_text() == (source.replace('paragraph\n/// continues', 'paragraph continues') if enabled else source)
    f.write_text(source)
    assert fastfmt(tmp_path, '--check').returncode == int(enabled)
