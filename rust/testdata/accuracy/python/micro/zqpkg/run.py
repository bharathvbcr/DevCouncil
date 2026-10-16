from .trainer import ZqHolderPlain, train_data, train_log, train_plain, train_slots


def run_plain():
    result = train_plain()
    return result.loss_log.losses()


def run_data():
    result = train_data()
    return result.loss_log.losses()


def run_slots():
    result = train_slots()
    return result.loss_log.losses()


def run_single():
    log = train_log()
    return log.losses()


def run_param(result: ZqHolderPlain):
    return result.loss_log.losses()


def run_param_plainlog(result: ZqHolderPlain):
    log = result.loss_log
    return log.losses()
