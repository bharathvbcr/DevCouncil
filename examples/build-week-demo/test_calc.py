"""Regression checks for the Build Week calculator demo."""

from calc import add, sub


def test_add() -> None:
    assert add(2, 3) == 5


def test_sub() -> None:
    assert sub(5, 3) == 2


def main() -> None:
    test_add()
    test_sub()
    print("ok")


if __name__ == "__main__":
    main()
