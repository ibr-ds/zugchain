#!/usr/bin/python

import re
import pandas as pd
import argparse

# pid rss cpu cmd
ps_re = re.compile(r'^\s*(\d+)\s+(\d+)\s+(\d+(:?\.\d+)?)\s+(.*)$')


def load_stats(file) -> pd.DataFrame:
    file = open(file, "r")
    content = file.readlines()

    rss = []
    cpu = []

    for line in content:
        if (match := ps_re.match(line)):
            rss.append(int(match.group(2)))
            cpu.append(float(match.group(3)))

    frame = pd.DataFrame({"rss": rss, "cpu": cpu})
    return frame

def mean_rss(frame: pd.DataFrame):
    return frame["rss"].mean()

def mean_cpu(frame: pd.DataFrame):
    return frame["cpu"].mean()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("file", type=str, metavar='F')

    args = parser.parse_args()

    frame = load_stats(args.file)

    print(frame)
    print(mean_rss(frame));
    print(mean_rss(frame) * 2)

    print(mean_cpu(frame));
    print(mean_cpu(frame) * 2)

