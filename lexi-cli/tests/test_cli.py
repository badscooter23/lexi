#!/usr/bin/env python3
"""Tests for CLI main functionality."""

import json
from pathlib import Path
from unittest.mock import patch

import pytest

from lexi_cli.cli import main


class TestCLI:
    """Test CLI entry points and basic functionality."""

    def test_help_command(self, capsys):
        """Test help command displays correctly."""
        code = main(["help"])
        assert code == 0
        captured = capsys.readouterr()
        assert "Lexi Commands:" in captured.out
        assert "General Commands:" in captured.out

    def test_version_command(self, capsys):
        """Test version command displays correctly."""
        code = main(["version"])
        assert code == 0
        captured = capsys.readouterr()
        assert "0.2.0" in captured.out

    @pytest.mark.integration
    def test_config_list(self, tmp_path, capsys):
        """Test config list command."""
        config_file = tmp_path / "config.json"
        config_file.write_text('{"test_key": "test_value"}')

        with patch('lexi_cli.cli.CONFIG_FILE', config_file):
            code = main(["config", "list"])
            assert code == 0
            captured = capsys.readouterr()
            assert "test_key" in captured.out

    def test_unknown_command(self, capsys):
        """Test unknown command returns error."""
        code = main(["nonexistent_command"])
        assert code == 1
        captured = capsys.readouterr()
        assert "Unknown command" in captured.err or "Unknown command" in captured.out


@pytest.fixture
def temp_config_dir(tmp_path):
    """Create temporary config directory for tests."""
    config_dir = tmp_path / ".lexi-cli"
    config_dir.mkdir()
    return config_dir


@pytest.fixture
def sample_config():
    """Sample configuration for tests."""
    return {
        "cli_name": "lexi",
        "version": "0.2.0",
        "prompt": "test",
        "prompt_delimiter": ">"
    }
