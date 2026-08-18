# Python Audit Blind-spot Syntax Fixture (G3, __all__ +=, stdlib methods)
import sys

__all__ = ["my_func"]
__all__ += ["MyClass"]

def open(filename: str):
    # G3: stdlib-named method/function
    return f"mock_open({filename})"

def dir():
    # G3: stdlib-named method
    return ["item1", "item2"]

def my_func():
    res = open("data.txt")
    return res

class MyClass:
    def execute(self):
        return dir()
