#!/usr/bin/env python3
"""Print the command names a shell script runs, one per line.

Used by test-initramfs-commands.sh to check the 90thinfloe dracut hooks against
the commands that actually exist in the built initrd. It is a heuristic
reader, not a shell parser: it finds the word in command position after
newlines, ; && || | ( ) { } ! and the shell keywords that start a command,
and descends into $( ... ) and backticks, including inside double quotes.
for/case headers and case patterns are skipped. Variable expansions and
anything that does not look like a command name are dropped; the caller
removes builtins and functions.
"""

import re
import shlex
import sys

NAME = re.compile(r"^[A-Za-z_][A-Za-z0-9_.+-]*$")
ASSIGN = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")
SEPARATORS = {";", "&&", "||", "|", "&", "(", ")", "{", "}", "!", "|&"}
STARTERS = {"then", "do", "else", "elif", "if", "while", "until", "time"}


def substitutions(text):
    """Yield the bodies of every $( ... ) and `...` in text."""
    i = 0
    while i < len(text):
        if text.startswith("$(", i) and not text.startswith("$((", i):
            depth, j = 1, i + 2
            while j < len(text) and depth:
                if text[j] == "(":
                    depth += 1
                elif text[j] == ")":
                    depth -= 1
                j += 1
            yield text[i + 2 : j - 1]
            i = j
        elif text[i] == "`":
            j = text.find("`", i + 1)
            if j < 0:
                break
            yield text[i + 1 : j]
            i = j + 1
        else:
            i += 1


def commands(text):
    out = []
    lex = shlex.shlex(text, posix=False, punctuation_chars=";&|()<>")
    lex.whitespace = " \t\r"  # keep newlines: they end a command
    lex.wordchars += "$[]{}*?/.:=-+,@%^~!#\\"
    lex.commenters = ""
    tokens = []
    try:
        for tok in lex:
            tokens.append(tok)
    except ValueError:
        pass

    expect_cmd = True
    skip_until = None  # "do" after for, "in" after case
    in_case = 0
    in_pattern = False
    for tok in tokens:
        for body in substitutions(tok):
            out.extend(commands(body))
        if tok.startswith("#") and expect_cmd:
            # rest of the line is a comment: shlex gave it to us word by word
            skip_until = "\n"
            continue
        if skip_until is not None:
            if tok == skip_until or (skip_until == "\n" and tok == "\n"):
                if skip_until == "in" and in_case:
                    in_pattern = True
                skip_until = None
                expect_cmd = True
            continue
        if in_pattern:
            if tok == ")":
                in_pattern = False
                expect_cmd = True
            elif tok == "esac":
                in_pattern = False
                in_case -= 1
            continue
        if tok == "\n" or tok in SEPARATORS:
            expect_cmd = True
            continue
        if tok == ";;":
            if in_case:
                in_pattern = True
            expect_cmd = True
            continue
        if tok == "esac":
            in_case = max(0, in_case - 1)
            continue
        if not expect_cmd:
            continue
        if tok in STARTERS:
            continue
        if tok in ("fi", "done"):
            expect_cmd = False
            continue
        if tok == "for":
            skip_until = "do"
            continue
        if tok == "case":
            in_case += 1
            skip_until = "in"
            continue
        if ASSIGN.match(tok):
            continue  # VAR=value prefix; the command, if any, follows
        if tok in ("<", ">", ">>", "<<", "2>", "&>"):
            continue
        expect_cmd = False
        if NAME.match(tok):
            out.append(tok)
    return out


def strip_comments(text):
    """Drop full-line comments and trailing ' # ...' comments."""
    lines = []
    for line in text.splitlines():
        stripped = line.lstrip()
        if stripped.startswith("#"):
            lines.append("")
            continue
        m = re.search(r"\s#\s", line)
        if m and line.count('"', 0, m.start()) % 2 == 0 and \
                line.count("'", 0, m.start()) % 2 == 0:
            line = line[: m.start()]
        lines.append(line)
    return "\n".join(lines) + "\n"


def main():
    seen = set()
    for path in sys.argv[1:]:
        with open(path) as f:
            text = strip_comments(f.read())
        for cmd in commands(text):
            if cmd not in seen:
                seen.add(cmd)
                print(cmd)


if __name__ == "__main__":
    main()
