from .base_b import SkBase


class SkChild(SkBase):
    def ping(self):
        return super().ping()
