"""P-ext (added for the dead-policy card, second pass): a method a library base
class calls by name. `HTMLParser.feed` calls `handle_starttag`; nothing in the
corpus does (Chronicle's import_knowledgebase.py)."""
from html.parser import HTMLParser


class TextParser(HTMLParser):
    def handle_starttag(self, tag, attrs):
        self.seen = tag


def to_text(html):
    parser = TextParser()
    parser.feed(html)
    return parser
