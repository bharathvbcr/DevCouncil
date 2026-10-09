from fastapi import FastAPI

from pkg.util import helper

app = FastAPI()


@app.get("/items")
def list_items():
    """C18 FastAPI route handler."""
    return [helper(1)]


@app.post("/items")
async def create_item():
    return {"ok": helper(2)}
