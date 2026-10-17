import extpkg

from .base_a import SkBase, SkLeft, SkRight, SkSilent
from .base_a import SkBase as AliasBase


class SkChild(SkBase):
    def ping(self):
        return super().ping()


class SkAliased(AliasBase):
    def ping(self):
        return super().ping()


class SkBuilder(SkBase):
    @classmethod
    def build(cls):
        return super().build()


class SkNested(SkBase):
    def ping(self):
        def inner():
            return super().ping()

        return inner


class SkDiamond(SkLeft, SkRight):
    def hop(self):
        return super().hop()


class SkOneProvider(SkSilent, SkLeft):
    def hop(self):
        return super().hop()


class SkExternalFirst(extpkg.Mixin, SkLeft):
    def hop(self):
        return super().hop()


class SkMeta(SkLeft, metaclass=SkMetaBase):
    def hop(self):
        return super().hop()


class SkMetaBase(type):
    def hop(cls):
        return 0
