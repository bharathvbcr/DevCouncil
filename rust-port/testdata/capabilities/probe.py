from helper import helper
import os


class Widget(BaseWidget):
    def render(self) -> str:
        return helper(self.name)


def main() -> None:
    w = Widget()
    w.render()
    os.getcwd()
