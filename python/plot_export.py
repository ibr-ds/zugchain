#!/usr/bin/env python

import argparse
import os
import json
import pandas as pd
import re
from pandas.core.frame import DataFrame

def lossy_json(content):
    for line in content:
        line = line.strip()
        try:
            yield json.loads(line)
        except:
            # print(f"line '{line}' is not json")
            pass

def atoi(text):
    return int(text) if text.isdigit() else text

def natural_keys(text):
    '''
    alist.sort(key=natural_keys) sorts in human order
    http://nedbatchelder.com/blog/200712/human_sorting.html
    (See Toothy's implementation in the comments)
    '''
    return [ atoi(c) for c in re.split(r'(\d+)', text) ]


def parse_deltas(frame: pd.DataFrame, column: str):
    frame[column] = frame[column].str.replace('µ', 'u')
    frame[column] = pd.to_timedelta(frame[column])

def read_export(file):
    with open(file, "r") as f:
        content = f.readlines()
    objects = [o for o in lossy_json(content)]
    table = pd.json_normalize(objects)

    parse_deltas(table, "time.idle")
    parse_deltas(table, "time.busy")

    return table


def load_export_run(root, name):
    run = []
    for root, dirs, files in os.walk(root):
        for dir in dirs:
            print(f'reading {root}/{dir}')
            try:
                frame = read_export(f"{root}/{dir}/{name}.log")
                run.append(frame)
            except Exception as e:
                print(f'Error {e}')
                continue
    return pd.concat(run)


def average(frame: pd.DataFrame, target: str, busy=False):
    values = frame[frame["target"] == target]
    if busy:
        column = "time.busy"
    else:
        column = "time.idle"

    avg = values[column].mean()
    std = values[column].std()

    return (avg, std)

def eval_run(run: pd.DataFrame, name: str):
    print(run)
    print(f"eval {name}")
    verify = average(run, "__measure::export::verify")
    fetch = average(run, "__measure::export::fetch")
    delete = average(run, "__measure::export::delete")

    verify_busy = average(run, "__measure::export::verify", busy=True)
    fetch_busy = average(run, "__measure::export::fetch", busy=True)
    delete_busy = average(run, "__measure::export::delete", busy=True)

    return {
        "name": name,
        "verify_mean": (verify_busy[0].value),
        "verify_std": (verify_busy[1]).value,
        "fetch_mean": (fetch[0] + fetch_busy[0]- verify_busy[0]).value,
        "fetch_std": (fetch[1] + fetch_busy[1] - verify_busy[1]).value,
        "delete_mean": (delete[0] + delete_busy[0]).value,
        "delete_std": (delete[1] + delete_busy[1]).value,
    }


def load_export(dir):
    for root, dirs, files in os.walk(dir):
        return [load_export_run(f"{dir}/{subdir}", "export-aws-vm1") for subdir in sorted(dirs, key=natural_keys)]

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("dir")
    parser.add_argument("--output", "-o")

    args = parser.parse_args()

    spec_file = open(f"{args.dir}/spec.json", "r")
    spec = json.load(spec_file)

    print(f"{spec['argument']}: {spec['values']}")

    series = load_export(args.dir)

    total = pd.DataFrame([eval_run(run, name) for (run, name) in zip(series, spec["values"])])
    total.set_index("name", inplace=True)
    total = total.transpose()
    print("results:")
    print(total)

    if args.output:
        total.to_csv(f"{args.output}/export.csv", index_label=spec["argument"])

