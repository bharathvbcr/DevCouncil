import asyncio
from pathlib import Path
from mcp.server import Server
from mcp.server.stdio import stdio_server
from mcp.types import Tool, TextContent
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import TaskRepository, ArtifactGraphRepository
from devcouncil.reporting.report_builder import ReportBuilder

app = Server("devcouncil")

@app.list_tools()
async def list_tools() -> list[Tool]:
    return [
        Tool(
            name="devcouncil_status",
            description="Get the current status of the DevCouncil project, including phase, tasks, and gaps.",
            inputSchema={
                "type": "object",
                "properties": {}
            }
        ),
        Tool(
            name="devcouncil_report",
            description="Get the full coverage report and a list of all requirements and blocking gaps.",
            inputSchema={
                "type": "object",
                "properties": {}
            }
        ),
        Tool(
            name="devcouncil_get_task",
            description="Get details, constraints, and requirements for a specific implementation task.",
            inputSchema={
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "The ID of the task, e.g. TASK-001"
                    }
                },
                "required": ["task_id"]
            }
        ),
    ]

@app.call_tool()
async def call_tool(name: str, arguments: dict) -> list[TextContent]:
    db = get_db(Path("."))
    if not db:
        return [TextContent(type="text", text="Error: DevCouncil not initialized in this directory.")]

    if name == "devcouncil_status":
        with db.get_session() as session:
            graph_repo = ArtifactGraphRepository(session)
            graph = graph_repo.load_graph()
            summary = graph.coverage_summary()
            
            status_str = "Phase: Determine from tasks/requirements\n"
            status_str += f"Requirements: {summary['total_requirements']} ({summary['requirements_without_tasks']} unmapped)\n"
            status_str += f"Tasks: {summary['total_tasks']} ({summary['tasks_without_requirements']} orphaned)\n"
            status_str += f"Gaps: {summary['total_gaps']} ({summary['blocking_gaps']} blocking)\n"
            
            return [TextContent(type="text", text=status_str)]

    elif name == "devcouncil_report":
        with db.get_session() as session:
            graph_repo = ArtifactGraphRepository(session)
            graph = graph_repo.load_graph()
            markdown_report = ReportBuilder.build_markdown(graph)
            return [TextContent(type="text", text=markdown_report)]
            
    elif name == "devcouncil_get_task":
        task_id = arguments.get("task_id")
        if not task_id:
            return [TextContent(type="text", text="Error: Missing task_id")]
            
        with db.get_session() as session:
            task_repo = TaskRepository(session)
            task = task_repo.get_by_id(task_id)
            if not task:
                return [TextContent(type="text", text=f"Error: Task {task_id} not found.")]
            
            return [TextContent(type="text", text=task.model_dump_json(indent=2))]

    return [TextContent(type="text", text=f"Unknown tool: {name}")]

async def run():
    # Use stdio to communicate
    async with stdio_server() as (read_stream, write_stream):
        await app.run(read_stream, write_stream, app.create_initialization_options())

if __name__ == "__main__":
    asyncio.run(run())
