from pkg.mod import helper

@app.get("/health")
def health():
    return helper()

if __name__ == "__main__":
    health()
