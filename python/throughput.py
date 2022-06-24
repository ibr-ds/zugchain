import pandas as pd
import numpy as np
import argparse
from load import load_log

def throughput(frame: pd.DataFrame) -> float:
    if frame.empty:
        return np.nan

    tp = frame[frame["target"] == "__measure::app"]
    tp = tp[tp["message"] == "execute"]
    start = tp.index.array[0]
    end = tp.index.array[-1]
    count = len(tp.index)-1
    delta = end-start

    # print(count)
    # print(delta)
    # print(tp)

    return count / (delta.value / 1_000_000_000)

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("file", type=str)

    args = parser.parse_args()

    log = load_log(args.file)

    frame = throughput(log)
    print(frame)