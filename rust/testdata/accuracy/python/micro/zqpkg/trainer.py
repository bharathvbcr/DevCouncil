from dataclasses import dataclass


class ZqLog:
    def losses(self):
        return []


class ZqLogPlain:
    def losses(self):
        return [1]


class ZqLogSlots:
    def losses(self):
        return [2]


class ZqHolderPlain:
    loss_log: ZqLogPlain


@dataclass
class ZqHolderData:
    loss_log: ZqLog


@dataclass(frozen=True, slots=True)
class ZqHolderSlots:
    loss_log: ZqLogSlots


def train_plain() -> ZqHolderPlain:
    return ZqHolderPlain()


def train_data() -> ZqHolderData:
    return ZqHolderData()


def train_slots() -> ZqHolderSlots:
    return ZqHolderSlots()


def train_log() -> ZqLogPlain:
    return ZqLogPlain()


class ZqHolderGeneric:
    loss_logs: list[ZqLog]


def train_generic() -> ZqHolderGeneric:
    return ZqHolderGeneric()
