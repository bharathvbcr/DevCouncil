from shapes import Shape, Square


def summarise(shape: Shape):
    return shape.area()


def run():
    return summarise(Square(3))


run()
