#!/usr/bin/python

import argparse
from load import load_log
import pandas as pd
import numpy as np
import matplotlib.pyplot as plt

def parse_deltas(frame: pd.DataFrame, column: str):
    frame[column] = frame[column].str.replace('µ', 'u')
    frame[column] = pd.to_timedelta(frame[column])

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("filename", default="../benchmarks/sim-viewchange-2020-12-09T07:55:13.728001286+00:00/railchain-mcom2.log", nargs="?")
    parser.add_argument("--output", "-o")

    args = parser.parse_args()

    print(args)

    log = load_log(args.filename)

    vc = log.loc[(log["target"] == "__measure::vc") & (log["message"] == "close")]
    print(vc)

    vc_time = vc.index.values[0]
    after_vc = vc_time + pd.Timedelta("10s")
    before_vc = vc_time - pd.Timedelta("10s")

    print(vc_time, after_vc, before_vc)

    parse_deltas(log, "time.idle")

    latencies = log[log["target"] == "__measure::request"]["time.idle"]
    latencies = latencies.loc[(latencies.index >= before_vc) & (latencies.index <= after_vc)]

    latencies = latencies.reset_index()
    latencies["time.idle"] = latencies["time.idle"].apply(lambda t: t.value)


    latencies.set_index("timestamp", inplace=True)
    # latencies = latencies.resample("256ms").mean()

    plt.plot(latencies)
    plt.show()

    latencies = latencies.reset_index()
    latencies["timestamp"] = latencies["timestamp"].apply(lambda t: t.value)

    if args.output:
        out = latencies.transpose()
        out.to_csv(f"{args.output}", index_label="index")
# %%
