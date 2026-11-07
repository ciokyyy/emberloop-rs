Search the compressed history: the summaries and topics of compressed blocks, and the original turns folded into them. Visible messages are not searched.

`query` is one or more words. Each word is matched as a case-insensitive substring, and a block scores by how often its words occur, so blocks with more of the words rank first. `limit` caps the results (default 5).

Each hit gives the block id, its topic and a snippet around the rarest matched word, which is marked 【】. A hit marked inactive was restored already, so its turns are back in the history. A hit listed under original text is not in any summary; its turns hold the text.

Search here first, then `decompress` the block id to read its full record.
