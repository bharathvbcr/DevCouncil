from typing import List, Dict, Any, Type, Optional
import copy
import json
import logging
import asyncio
from pathlib import Path

from pydantic import BaseModel
from devcouncil.llm.provider import Provider
from devcouncil.llm.cache import LLMCache
from devcouncil.telemetry.tracker import TelemetryTracker

logger = logging.getLogger(__name__)

class ModelRouter:
    def __init__(self, provider: Provider, role_config: Dict[str, Dict[str, Any]]):
        self.provider = provider
        self.role_config = role_config

    async def complete_structured(
        self,
        role: str,
        messages: List[Dict[str, str]],
        schema: Type[BaseModel],
        temperature: Optional[float] = None,
        run_id: Optional[str] = None,
    ) -> BaseModel:
        config = self.role_config.get(role)
        if not config:
            raise ValueError(f"No config found for role: {role}")
        
        model = config["model"]
        temp = temperature if temperature is not None else config.get("temperature", 0.0)
        
        # Deep-copy to avoid mutating the caller's messages list
        msgs = copy.deepcopy(messages)
        
        # Add schema instructions to system or user message
        schema_json = json.dumps(schema.model_json_schema(), indent=2)
        instruction = f"\n\nYou MUST output a JSON object matching this schema:\n{schema_json}"
        
        found_system = False
        for msg in msgs:
            if msg["role"] == "system":
                msg["content"] += instruction
                found_system = True
                break
        
        if not found_system:
            msgs.insert(0, {"role": "system", "content": f"You are a helpful assistant.{instruction}"})

        logger.info("LLM call: role=%s model=%s run_id=%s", role, model, run_id)

        project_root = Path(".")
        cache = LLMCache(project_root)
        tracker = TelemetryTracker(project_root)

        # Check cache first
        response = cache.get(model, msgs, temp, True)
        cache_hit = response is not None

        if not response:
            for attempt in range(3):
                try:
                    response = await self.provider.complete(
                        model=model,
                        messages=msgs,
                        temperature=temp,
                        json_mode=True
                    )
                    cache.set(model, msgs, temp, True, response)
                    break
                except Exception as e:
                    if attempt == 2:
                        raise
                    logger.warning(f"LLM request failed (attempt {attempt+1}): {e}. Retrying...")
                    await asyncio.sleep(2 ** attempt)

        if not cache_hit:
            tracker.log_usage(model, response.usage)

        logger.info(
            "LLM response: role=%s model=%s tokens=%s",
            role, response.model, response.usage,
        )
        
        try:
            # Attempt to find JSON block if it's wrapped in markdown
            content = response.content.strip()
            if "```json" in content:
                content = content.split("```json")[1].split("```")[0].strip()
            elif "```" in content:
                content = content.split("```")[1].split("```")[0].strip()
                
            data = json.loads(content)
            return schema.model_validate(data)
        except Exception as e:
            logger.warning(f"Initial parse failed for {role}, attempting healing: {e}")
            
            # Healing attempt: Ask the model to fix its own JSON
            healing_prompt = f"""
The following JSON was returned but failed to parse or validate against the schema.
Error: {str(e)}
Content:
{response.content}

Please return the corrected JSON object only. No prose.
"""
            # We use a lower temperature for healing
            healed_response = await self.provider.complete(
                model=model,
                messages=[{"role": "user", "content": healing_prompt}],
                temperature=0.0,
                json_mode=True
            )
            
            try:
                healed_content = healed_response.content.strip()
                if "```json" in healed_content:
                    healed_content = healed_content.split("```json")[1].split("```")[0].strip()
                data = json.loads(healed_content)
                return schema.model_validate(data)
            except Exception as final_e:
                logger.error(f"Healing failed for {role}: {final_e}")
                raise ValueError(f"Failed to parse or validate LLM response after healing: {final_e}\nContent (truncated): {response.content[:200]}...")
