# RIBBIT RECOIL build project

This is the buildable multi-file form of the game. The Lua, sprite sheet, and map are extracted from carts/ribbit-recoil.cart; audio/game.cmusic supplies the six-channel Operation Lilybreaker mix plus remapped gameplay cues.

console music play audio/game.cmusic --song 0 auditions the native bundle.

console build . creates build/ribbit-recoil.cart from the project sources.
console build . --check verifies that generated cart on a second pass; the output is ignored because the source files and bundle are authoritative.
