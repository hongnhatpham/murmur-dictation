from murmur.cli import main


def test_dictionary_cli_add_list_remove(tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "config"))

    assert main(["dictionary", "add", "Niri"]) == 0
    assert main(["dictionary", "list"]) == 0
    assert "Niri" in capsys.readouterr().out
    assert main(["dictionary", "remove", "Niri"]) == 0
    assert main(["dictionary", "list"]) == 0
    assert "Niri" not in capsys.readouterr().out


def test_snippet_cli_add_expand_remove(tmp_path, monkeypatch, capsys):
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "config"))

    assert main(["snippets", "add", ";sig", "Regards,", "Murmur"]) == 0
    assert main(["snippets", "expand", ";sig"]) == 0
    assert "Regards, Murmur" in capsys.readouterr().out
    assert main(["snippets", "remove", ";sig"]) == 0
