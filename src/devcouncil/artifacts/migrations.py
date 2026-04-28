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
        return data
