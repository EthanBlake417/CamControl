"""Image processing tools (no GUI code here).

Every tool takes plain numpy images (8-bit BGR or grayscale), so it works
the same on live captures and on files, including full-size SD card images.
Each module can be run directly for a self-test on made-up images:

    python -m camcontrol.processing.focus_stack
"""
