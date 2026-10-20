from .base import make_table, make_table_list, unannotated_pair


def use_call_result():
    return make_table([1]).digest_of()


def use_function_attribute():
    return make_table.cache_clear()


def use_tuple_unpack():
    table, other = unannotated_pair()
    return table.digest_of()


def use_call_in_arguments():
    return make_table.cache_clear(make_table(2))


def use_generic_result():
    return make_table_list([1]).digest_of()


def use_shadowed_factory(make_table):
    return make_table([1]).digest_of()
