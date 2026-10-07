class Shape:
    def area(self):
        raise NotImplementedError


class Square(Shape):
    def __init__(self, side):
        self.side = side

    def area(self):
        return self.side * self.side

    def perimeter(self):
        return 4 * self.side


class Circle:
    radius = 1.0

    def area(self):
        return 3.14159 * self.radius * self.radius
