"""Rebuild the original test-only block font with fonttools==4.58.5.

Every printable character deliberately uses one simple geometric glyph. This
font contains no third-party glyphs; it tests embedded-font selection, not text.
"""
from pathlib import Path
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen

builder = FontBuilder(1000, isTTF=True)
builder.setupGlyphOrder([".notdef", "space", "block"])
builder.setupCharacterMap({code: "block" for code in range(33, 127)} | {32: "space"})
glyphs = {}
for name in [".notdef", "space", "block"]:
    pen = TTGlyphPen(None)
    if name == "block":
        pen.moveTo((50, 0)); pen.lineTo((550, 0)); pen.lineTo((550, 800)); pen.lineTo((50, 800)); pen.closePath()
    glyphs[name] = pen.glyph()
builder.setupGlyf(glyphs)
builder.setupHorizontalMetrics({name: (600, 0) for name in glyphs})
builder.setupHorizontalHeader(ascent=800, descent=-200)
builder.setupNameTable({"familyName": "Jesses Fixture", "styleName": "Regular", "uniqueFontIdentifier": "Jesses Fixture 1", "fullName": "Jesses Fixture Regular", "psName": "JessesFixture-Regular", "version": "Version 1.0"})
builder.setupOS2(sTypoAscender=800, sTypoDescender=-200, usWinAscent=800, usWinDescent=200)
builder.setupPost()
builder.setupMaxp()
builder.font["head"].created = builder.font["head"].modified = 2082844800
builder.save(Path(__file__).with_name("subtitle-font.ttf"))
