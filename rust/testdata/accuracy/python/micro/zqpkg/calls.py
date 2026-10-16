from .base import make_table, unannotated_pair


def use_call_result():
    return make_table([1]).digest_of()


def use_function_attribute():
    return make_table.cache_clear()


def use_tuple_unpack():
    table, other = unannotated_pair()
    return table.digest_of()
