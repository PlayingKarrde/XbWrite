XbWrite @VERSION@ for @PLATFORM@
==================================

XbWrite turns original Xbox disc images into trimmed XISO images. It rebuilds each
game from its files alone, without the DVD video partition and the filler a full
disc dump carries, so the image is only as big as the game.

What's in this package
----------------------

@CONTENTS@
@NOTES@
Using it
--------

1. Drag Xbox disc images or game folders onto the window, or use Add files or
   Add folder. Redump ISOs, XISOs, CSOs, split images (.1.iso, .2.iso, ...) and
   extracted game folders (with a default.xbe) all work.
2. Check the game cards: each shows the size now and after converting.
3. Choose XISO or CSO at the bottom, then click Convert. The new image is saved
   next to the source unless you pick an output folder at the bottom of the window.

XISO works in xemu and every Xbox loader. CSO is smaller but only read by some
loaders on the console (Cerbios, Stellar, XBMC4Gamers), not by xemu.

Settings
--------

- Drop the dashboard update (on): leaves out the $SystemUpdate folder.
- Split for FAT32 (off): cuts images over 4 GB into numbered parts.
- Verify after writing (on): reads the new image back and compares every file.
- Replace the original image (off): deletes the source image after a successful
  conversion. Game folders are never deleted.

Extracting to a folder
----------------------

Click Extract... on a game card, pick a destination and click Extract. The game's
files go to a folder named after the image.

Command line
------------

  xbwrite-cli info <images or folders...>
  xbwrite-cli convert [-o DIR] [--cso] [--split] [--keep-update] [--no-verify] [--replace] <images or folders...>
  xbwrite-cli extract [-o DIR] [--keep-update] [--overwrite] <images...>

Credits
-------

XbWrite builds images with xdvdfs by antangelo (https://github.com/antangelo/xdvdfs),
licensed under MIT (see LICENSE-xdvdfs.txt). XbWrite is not affiliated with
Microsoft. Licensed under MIT (see LICENSE.txt).
