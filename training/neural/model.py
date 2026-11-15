"""Position-wise bopomofo -> character model.

Input per position: either a known character (left context) or a syllable to
convert. One forward pass of a bidirectional Transformer encoder scores, for
every syllable position, the characters that syllable can stand for. There is
no autoregressive generation.

A syllable is embedded as base (initial+medial+final) plus tone, so the tone
can be withheld ("tone unknown") both in training and for toneless typing.
"""

from __future__ import annotations

import math
from dataclasses import asdict, dataclass

import torch
import torch.nn as nn
import torch.nn.functional as F

TONES = ["", "ˊ", "ˇ", "ˋ", "˙"]  # index 5 = unknown tone
TONE_UNKNOWN = 5


def split_syllable(syl: str) -> tuple[str, int]:
    """'ㄓㄨㄥˋ' -> ('ㄓㄨㄥ', 3); '˙ㄉㄜ' -> ('ㄉㄜ', 4); first tone is 0."""
    if syl.startswith("˙"):
        return syl[1:], 4
    if syl and syl[-1] in "ˊˇˋ":
        return syl[:-1], TONES.index(syl[-1])
    return syl, 0


class Tables:
    """Id tables derived from vocab.json (shared by training and inference)."""

    def __init__(self, vocab: dict):
        self.chars: list[str] = vocab["chars"]
        self.syllables: list[str] = vocab["syllables"]
        self.char_id = {c: i for i, c in enumerate(self.chars)}
        self.syl_id = {s: i for i, s in enumerate(self.syllables)}
        self.mask_char = len(self.chars)  # input id of a position to be converted
        bases = sorted({split_syllable(s)[0] for s in self.syllables[1:]})
        self.bases = ["<none>"] + bases
        base_id = {b: i for i, b in enumerate(self.bases)}
        self.syl_base = torch.zeros(len(self.syllables), dtype=torch.long)
        self.syl_tone = torch.full((len(self.syllables),), TONE_UNKNOWN, dtype=torch.long)
        # allowed[s] / allowed_base[b]: which characters a syllable (or a toneless base) may become
        self.allowed = torch.zeros(len(self.syllables), len(self.chars), dtype=torch.bool)
        self.allowed_base = torch.zeros(len(self.bases), len(self.chars), dtype=torch.bool)
        for i, syl in enumerate(self.syllables[1:], 1):
            base, tone = split_syllable(syl)
            self.syl_base[i], self.syl_tone[i] = base_id[base], tone
            ids = [self.char_id[c] for c in vocab["candidates"].get(syl, "")]
            self.allowed[i, ids] = True
            self.allowed_base[base_id[base], ids] = True


@dataclass
class Config:
    n_chars: int
    n_bases: int
    d_model: int = 384
    n_layers: int = 6
    n_heads: int = 6
    d_ff: int = 1536
    max_len: int = 96
    dropout: float = 0.1


class Block(nn.Module):
    def __init__(self, cfg: Config):
        super().__init__()
        self.n_heads = cfg.n_heads
        self.norm1 = nn.LayerNorm(cfg.d_model)
        self.qkv = nn.Linear(cfg.d_model, 3 * cfg.d_model)
        self.proj = nn.Linear(cfg.d_model, cfg.d_model)
        self.norm2 = nn.LayerNorm(cfg.d_model)
        self.ff1 = nn.Linear(cfg.d_model, cfg.d_ff)
        self.ff2 = nn.Linear(cfg.d_ff, cfg.d_model)
        self.drop = nn.Dropout(cfg.dropout)

    def forward(self, x: torch.Tensor, attn_mask: torch.Tensor) -> torch.Tensor:
        b, n, d = x.shape
        q, k, v = self.qkv(self.norm1(x)).view(b, n, 3, self.n_heads, d // self.n_heads).permute(2, 0, 3, 1, 4)
        a = F.scaled_dot_product_attention(q, k, v, attn_mask=attn_mask,
                                           dropout_p=self.drop.p if self.training else 0.0)
        x = x + self.drop(self.proj(a.transpose(1, 2).reshape(b, n, d)))
        return x + self.drop(self.ff2(F.gelu(self.ff1(self.norm2(x)))))


class Converter(nn.Module):
    def __init__(self, cfg: Config):
        super().__init__()
        self.cfg = cfg
        self.char_emb = nn.Embedding(cfg.n_chars + 1, cfg.d_model)  # +1: "to be converted"
        self.base_emb = nn.Embedding(cfg.n_bases, cfg.d_model)
        self.tone_emb = nn.Embedding(TONE_UNKNOWN + 1, cfg.d_model)
        # Positions are counted from the end of the sequence (the cursor), so the
        # syllable being typed always sits at position 0.
        self.pos_emb = nn.Embedding(cfg.max_len, cfg.d_model)
        self.drop = nn.Dropout(cfg.dropout)
        self.blocks = nn.ModuleList(Block(cfg) for _ in range(cfg.n_layers))
        self.norm = nn.LayerNorm(cfg.d_model)
        self.out_bias = nn.Parameter(torch.zeros(cfg.n_chars))
        self.apply(self._init)

    @staticmethod
    def _init(m: nn.Module) -> None:
        if isinstance(m, (nn.Linear, nn.Embedding)):
            nn.init.normal_(m.weight, std=0.02)
            if isinstance(m, nn.Linear) and m.bias is not None:
                nn.init.zeros_(m.bias)

    def config_dict(self) -> dict:
        return asdict(self.cfg)

    def forward(self, chars: torch.Tensor, bases: torch.Tensor, tones: torch.Tensor,
                lengths: torch.Tensor) -> torch.Tensor:
        """chars/bases/tones: [B, N] (left-aligned, padded); lengths: [B].
        Returns the hidden states [B, N, d]; pass the ones to convert to `logits`."""
        b, n = chars.shape
        idx = torch.arange(n, device=chars.device)
        from_end = (lengths[:, None] - 1 - idx[None, :]).clamp(0, self.cfg.max_len - 1)
        pad = idx[None, :] >= lengths[:, None]
        x = self.char_emb(chars) + self.pos_emb(from_end)
        is_syl = bases > 0
        x = x + (self.base_emb(bases) + self.tone_emb(tones)) * is_syl[..., None]
        x = self.drop(x)
        attn_mask = (~pad)[:, None, None, :]  # nobody attends to padding
        for blk in self.blocks:
            x = blk(x, attn_mask)
        return self.norm(x)

    def logits(self, hidden: torch.Tensor) -> torch.Tensor:
        """Character scores for selected positions: [..., d] -> [..., n_chars]."""
        return hidden @ self.char_emb.weight[: self.cfg.n_chars].T + self.out_bias

    def n_params(self) -> int:
        return sum(p.numel() for p in self.parameters())
