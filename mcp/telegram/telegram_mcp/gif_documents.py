"""Retain the document metadata needed to send integer GIF search results."""

from collections import OrderedDict
from weakref import WeakKeyDictionary

from telethon.tl.types import Document

MAX_GIF_DOCUMENTS = 100


class GifDocuments:
    def __init__(self):
        self._clients: WeakKeyDictionary[object, OrderedDict[int, Document]] = WeakKeyDictionary()

    def remember(self, client: object, documents: list[Document]) -> None:
        saved = self._clients.setdefault(client, OrderedDict())
        for document in documents:
            saved[document.id] = document
            saved.move_to_end(document.id)
        while len(saved) > MAX_GIF_DOCUMENTS:
            saved.popitem(last=False)

    def get(self, client: object, document_id: int) -> Document | None:
        saved = self._clients.get(client)
        if saved is None:
            return None
        document = saved.get(document_id)
        if document is not None:
            saved.move_to_end(document_id)
        return document

    def forget(self, client: object, document_id: int) -> None:
        saved = self._clients.get(client)
        if saved is not None:
            saved.pop(document_id, None)
