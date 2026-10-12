#!/usr/bin/env python3
"""Writes the byte streams the emulator is checked against. Each `<name>.<rows>x<cols>.in` is the
input of one test; `regen.sh` makes the matching `.out` with the daemon's own vt100."""
import os

E = "\x1b"
here = os.path.dirname(os.path.abspath(__file__))


def w(name, rows, cols, data):
    if isinstance(data, str):
        data = data.encode()
    with open(os.path.join(here, f"{name}.{rows}x{cols}.in"), "wb") as f:
        f.write(data)


w("sgr", 8, 44,
  f"plain {E}[1mbold{E}[0m {E}[2mdim{E}[22m {E}[3mital{E}[23m {E}[4muline{E}[24m {E}[7minv{E}[27m\r\n"
  f"{E}[31mred{E}[32m grn{E}[33m yel{E}[34m blu{E}[35m mag{E}[36m cyn{E}[37m wht{E}[39m dflt\r\n"
  f"{E}[91mbred{E}[92m bgrn{E}[0m {E}[41mbgred{E}[102m bgbgrn{E}[49m none\r\n"
  f"{E}[38;5;208morange{E}[48;5;17m onblue{E}[0m {E}[38;2;10;200;30mrgb{E}[48;2;100;0;50m bg{E}[0m\r\n"
  f"{E}[38;2;1;2;3mrgb2{E}[0m {E}[38:5:99mcolon5{E}[0m {E}[1;4;31;47mmany{E}[m reset\r\n"
  f"{E}[1mb{E}[22;31mx{E}[0m{E}[7;32mi{E}[0m end")

w("cursor", 8, 30,
  f"{E}[2;5Hat 2,5{E}[4;1Hrow4{E}[A^up{E}[2Bdn2{E}[5Cfwd{E}[3Dback"
  f"{E}[7;10H{E}7saved{E}[1;1Hhome{E}8{E}[1Gcol1{E}[3d@row3{E}[2;3HHVP"
  f"\r\n\ttab\ttab2<-bt{E}[E{E}[Fnext{E}[0;0Hzero{E}[99;99Hcorner")

w("erase", 6, 20,
  "".join(f"{E}[{r};1H" + "abcdefghijklmnopqrst" for r in range(1, 7)) +
  f"{E}[1;5H{E}[0K{E}[2;5H{E}[1K{E}[3;5H{E}[2K{E}[4;3H{E}[3X"
  f"{E}[44m{E}[5;10H{E}[0J{E}[0m{E}[6;1H{E}[41m{E}[2K{E}[0m{E}[6;1H{E}[1J")

w("lines", 8, 20,
  "".join(f"line{r}\r\n" for r in range(1, 8)) + "last" +
  f"{E}[3;1H{E}[2L{E}[5;1H{E}[M{E}[2;1H{E}[2@xx{E}[2P{E}[1;1H{E}[S{E}[T"
  f"{E}[2;6r{E}[6;1H\n\n{E}[2;1H{E}M{E}M{E}[r{E}[8;1Hend")

w("region", 6, 12,
  "".join(f"r{r}\r\n" for r in range(1, 6)) + "r6" +
  f"{E}[2;4r{E}[4;1H\nA\nB{E}[2;1H{E}Mtop{E}[3;1H{E}[L{E}[4;1H{E}[M{E}[?6h{E}[1;1Horig{E}[?6l")

w("wrap", 4, 10,
  "0123456789ABC\r\n" + "xyz" + f"{E}[1;10Hq" + "w" + 
  f"{E}[4;1Habz\r\n{E}[4;9Hhi")

w("wide", 4, 12,
  "中文a\r\n" + "é é x" + "\r\n" + "😀b😀" + f"{E}[1;2Hx{E}[2;1H" + "表" + f"{E}[4;11H表z")

w("alt", 5, 20,
  f"main text\r\n{E}[1;32mgreen{E}[0m\r\n{E}[?1049h{E}[2;2Halt!{E}[1;1Hx{E}[?25l"
  f"{E}[?1049l{E}[5;1Hback")

w("alt2", 5, 20,
  f"keep me{E}[3;5H{E}[?1049h{E}[H{E}[2Jinside alt{E}[3;3Hzz")

w("modes", 3, 10,
  f"{E}[?1h{E}={E}[?2004h{E}[?1000h{E}[?1006h{E}[?25l{E}[?1002hx")

w("modes_off", 3, 10,
  f"{E}[?1h{E}={E}[?2004h{E}[?1000h{E}[?1006h{E}[?1l{E}>{E}[?2004l{E}[?1000l{E}[?1006l{E}[?25lx{E}[?25h")


w("scroll", 5, 10, "".join(f"line {i}\r\n" for i in range(1, 21)) + "end")



w("reset", 4, 10, f"hello{E}[1;31m{E}[?25l{E}c" + "x")

w("utf8", 3, 20, "café üñ € 한글 ✓ \U0001f600".encode())
