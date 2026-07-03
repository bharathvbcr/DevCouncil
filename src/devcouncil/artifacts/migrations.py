from typing import Any, Dict

class ArtifactMigrator:
    """Migrates artifact schemas across DevCouncil versions."""
    
    @staticmethod
    def migrate_requirement(data: Dict[str, Any]) -> Dict[str, Any]:
        """Upgrade requirement payload to current schema."""
        if "priority" not in data:
            data["priority"] = "medium"
        return data

    @staticmethod
    def migrate_task(data: Dict[str, Any]) -> Dict[str, Any]:
        """Upgrade task payload to current schema."""
        if "forbidden_changes" not in data:
            data["forbidden_changes"] = []
        if "expected_tests" not in data:
            data["expected_tests"] = []
        if "agent_appended_expected_tests" not in data:
            data["agent_appended_expected_tests"] = []
        if "agent_appended_allowed_commands" not in data:
            data["agent_appended_allowed_commands"] = []
        if "difficulty" not in data:
            data["difficulty"] = None
        return data
