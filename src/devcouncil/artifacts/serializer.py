import json
from typing import Any, Dict, TypeVar, Type
from pydantic import BaseModel

T = TypeVar("T", bound=BaseModel)

class ArtifactSerializer:
    """Handles serialization and deserialization of DevCouncil artifacts."""
    
    @staticmethod
    def to_json(artifact: BaseModel) -> str:
        return artifact.model_dump_json(indent=2)

    @staticmethod
    def from_json(json_str: str, model_class: Type[T]) -> T:
        data = json.loads(json_str)
        return model_class.model_validate(data)

    @staticmethod
    def to_dict(artifact: BaseModel) -> Dict[str, Any]:
        return artifact.model_dump()
