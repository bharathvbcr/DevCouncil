"""Tests for the secret redaction utility."""

from devcouncil.utils.redaction import redact_text, redact_env_vars


class TestRedaction:
    def test_jwt_redaction(self):
        text = "token: eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.abcdef123456"
        result = redact_text(text)
        assert "eyJ" not in result
        assert "[REDACTED:jwt]" in result

    def test_aws_key_redaction(self):
        text = "aws_access_key = AKIAIOSFODNN7EXAMPLE"
        result = redact_text(text)
        assert "AKIAIOSFODNN7EXAMPLE" not in result
        assert "[REDACTED:" in result

    def test_private_key_redaction(self):
        text = """
-----BEGIN RSA PRIVATE KEY-----
MIIEowIBAAKCAQEA...
-----END RSA PRIVATE KEY-----
        """
        result = redact_text(text)
        assert "MIIEow" not in result
        assert "[REDACTED:private_key]" in result

    def test_database_url_redaction(self):
        text = "DATABASE_URL=postgresql://user:pass@host:5432/db"
        result = redact_text(text)
        assert "pass@host" not in result
        assert "[REDACTED:" in result

    def test_bearer_token_redaction(self):
        text = "Authorization: Bearer sk_test_abcdef1234567890abcdef"
        result = redact_text(text)
        assert "sk_test_" not in result
        assert "[REDACTED:bearer]" in result

    def test_plain_text_unchanged(self):
        text = "This is a normal function that adds two numbers together."
        result = redact_text(text)
        assert result == text

    def test_custom_patterns(self):
        text = "My internal ID is INT-12345-SECRET"
        result = redact_text(text, extra_patterns=[r"INT-\d+-SECRET"])
        assert "INT-12345-SECRET" not in result
        assert "[REDACTED:custom_0]" in result

    def test_env_var_redaction(self):
        text = "export OPENROUTER_API_KEY=sk-or-v1-abc123def456"
        result = redact_env_vars(text)
        assert "sk-or-v1-abc123def456" not in result
        assert "OPENROUTER_API_KEY=[REDACTED]" in result

    def test_env_var_password_redaction(self):
        text = "DB_PASSWORD='my_super_secret_pass'"
        result = redact_env_vars(text)
        assert "my_super_secret_pass" not in result
