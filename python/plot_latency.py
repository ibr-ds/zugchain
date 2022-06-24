#!/usr/bin/env python

from typing import List, Tuple

import numpy as np
import pandas as pd
import os
import matplotlib.pyplot as plt
import json
import argparse
import re

from load import load_log
from throughput import throughput
from process import load_stats, mean_rss, mean_cpu

def atoi(text):
    return int(text) if text.isdigit() else text

def natural_keys(text):
    '''
    alist.sort(key=natural_keys) sorts in human order
    http://nedbatchelder.com/blog/200712/human_sorting.html
    (See Toothy's implementation in the comments)
    '''
    return [ atoi(c) for c in re.split(r'(\d+)', text) ]

def timedelta_series(table: pd.DataFrame, delta_column: str,
                     filter_column: str, filter_value: str) -> pd.DataFrame:
    table[delta_column] = table[delta_column].str.replace('µ', 'u')
    table[delta_column] = pd.to_timedelta(table[delta_column])
    return table[table[filter_column] == filter_value]


def vc_latency(frame: pd.DataFrame) -> Tuple[float, float]:
    if frame.empty:
        return (np.nan, np.nan)

    vcs = frame[frame["target"] == "__measure::vc"]
    vcs = vcs[vcs["message"] == "close"]

    mean = vcs["time.idle"].mean()
    std = vcs["time.idle"].std()

    return (mean.value, std.value)

def nv_size(frame: pd.DataFrame) -> float:
    if frame.empty:
        return np.nan

    if not "prepares" in frame.columns:
        return np.nan

    vcs = frame[frame["target"] == "__measure::vc"]
    vcs = vcs[vcs["message"] == "nv"]

    return vcs["prepares"].mean()

def soft_timeoues(frame: pd.DataFrame) -> int:
    if frame.empty:
        return 0

    st = frame[frame["target"] == "__measure::soft_timeout"]
    return len(st.index)

def latency(frame: pd.DataFrame):
    if frame.empty:
        return (np.nan, np.nan)

    requests = timedelta_series(
        frame, "time.idle", "target", "__measure::request")
    mean = requests["time.idle"].mean()
    std = requests["time.idle"].std()
    return (mean.value, std.value)

# def vc_latency(frame: pd.DataFrame) -> float:
#     vcs = timedelta_series(
#         frame, "time.idle", "target", "__measure::vc")
#     return vcs["time.idle"].mean()

class Run:
    logs: pd.DataFrame
    stats: pd.DataFrame

    def __init__(self, logfile, statsfile):
        try:
            self.logs = load_log(logfile)
        except:
            self.logs = pd.DataFrame()
        self.stats = load_stats(statsfile)

def parse_run(dir: str, name: str, hosts: List[str]) -> List[pd.DataFrame]:
    logs = []
    for host in hosts:
        statsfile = f'{dir}/stats-{host}.txt'
        logfile = f'{dir}/{name}-{host}.log'
        run = Run(logfile, statsfile)
        logs.append(run)
    return logs


def open_series(dir: str, name: str, hosts: List[str]) -> List[List[Run]]:
    frames = []
    for root, dirs, _files in os.walk(dir):
        print(f"walking {root}")
        for dir in sorted(dirs, key=natural_keys):
            print(f"dirs {dir}")
            frames.append(parse_run(f'{root}/{dir}', name, hosts))
    return frames

def make_frame(spec, values, columns) -> pd.DataFrame:
    frame = pd.concat(values, ignore_index=True)
    frame.columns = columns
    frame[spec["argument"]] = spec["values"]
    frame.set_index(spec["argument"], inplace=True)
    return frame.transpose()

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="process railchain logs")
    parser.add_argument('--directory', '-d', type=str)
    parser.add_argument('--name', '-n', type=str)
    parser.add_argument('--outdir', '-o', type=str)
    parser.add_argument('--hosts', type=str, default="mcoms")
    args = parser.parse_args()
    print(args)

    dir = args.directory
    name = args.name

    if args.name == "baseline-server":
        stats_factor = 2
    else:
        stats_factor = 1

    if args.hosts == "mcoms":
        hosts = ['mcom1', 'mcom2', 'mcom3', 'mcom4']
    elif args.hosts == "rpis":
        hosts = ['pi1', 'pi2', 'pi3', 'pi4']
    else:
        hosts = ['mcom1', 'mcom2', 'mcom3', 'mcom4']

    spec_file = open(f"{dir}/spec.json", "r")
    spec = json.load(spec_file)
    spec["values"] = [int(value) for value in spec["values"]]

    print(spec)

    frames = open_series(dir, name, hosts)

    all_throughput = []
    all_latency_mean = []
    all_latency_std = []
    all_rss = []
    all_cpu = []
    all_vcs = []
    all_vcs_std = []
    all_nv = []
    all_st = []

    for run in frames:
        print(f'run {len(run)}')
        (means, stds) = zip(*[latency(node.logs) for node in run])
        frame = pd.DataFrame(means).transpose()
        all_latency_mean.append(frame)

        std_frame = pd.DataFrame(stds).transpose()
        all_latency_std.append(std_frame)


        tps = [throughput(node.logs) for node in run]
        tp_frame = pd.DataFrame(tps).transpose()
        all_throughput.append(tp_frame)

        rss = [mean_rss(node.stats) * stats_factor for node in run]
        all_rss.append(pd.DataFrame(rss).transpose())
        cpu = [mean_cpu(node.stats) * stats_factor for node in run]
        all_cpu.append(pd.DataFrame(cpu).transpose())

        (vc_means, vc_stds) = zip(*[vc_latency(node.logs) for node in run])
        all_vcs.append(pd.DataFrame(vc_means).transpose())
        all_vcs_std.append(pd.DataFrame(vc_stds).transpose())

        nv = [nv_size(node.logs) for node in run]
        all_nv.append(pd.DataFrame(nv).transpose())

        st = [soft_timeoues(node.logs) for node in run]
        all_st.append(pd.DataFrame(st).transpose())

    latencies = make_frame(spec, all_latency_mean, hosts)
    latency_std = make_frame(spec, all_latency_std, hosts)
    tps = make_frame(spec, all_throughput, hosts)
    cpu = make_frame(spec, all_cpu, hosts)
    rss = make_frame(spec, all_rss, hosts)
    vcs = make_frame(spec, all_vcs, hosts)
    vcs_std = make_frame(spec, all_vcs_std, hosts)
    nvs = make_frame(spec, all_nv, hosts)
    sts = make_frame(spec, all_st, hosts)

    print("latency")
    print(latencies)
    print("throughput")
    print(tps)
    print("cpu")
    print(cpu)
    print("memory")
    print(rss)

    print("vcs")
    print(vcs)

    print("nv size")
    print(nvs)

    print("soft timeouts")
    print(sts)

    outdir = args.outdir
    os.makedirs(outdir, exist_ok=True)

    tps_plot = tps.transpose()
    tps_plot["control"] = [1000 / x for x in tps_plot.index.values]
    plt.xticks(tps_plot.index.values)
    plt.plot(tps_plot)
    plt.ylim(bottom=0)
    # plt.legend(hosts)
    plt.show()


    latencies.to_csv(f"{outdir}/latecy_mean.csv", index_label=spec["argument"])
    latency_std.to_csv(f"{outdir}/latency_std.csv", index_label=spec["argument"])
    tps.to_csv(f"{outdir}/throughput.csv", index_label=spec["argument"])
    cpu.to_csv(f"{outdir}/cpu.csv", index_label=spec["argument"])
    rss.to_csv(f"{outdir}/memory.csv", index_label=spec["argument"])

    vcs.to_csv(f"{outdir}/vc_duration.csv", index_label=spec["argument"])
    vcs_std.to_csv(f"{outdir}/vc_dur_std.csv", index_label=spec["argument"])
    nvs.to_csv(f"{outdir}/nv_prepare_count.csv", index_label=spec["argument"])


