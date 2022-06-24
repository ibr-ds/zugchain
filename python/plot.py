# %%

import pandas as pd

from load import load_log


def timedelta_series(table: pd.DataFrame, delta_column: str,
                     filter_column: str, filter_value: str) -> pd.DataFrame:
    table[delta_column] = table[delta_column].str.replace('µ', 'u')
    table[delta_column] = pd.to_timedelta(table[delta_column])
    return table[table[filter_column] == filter_value]


def view_changes(table: pd.DataFrame):
    vc = table[table["target"] == "__measure::vc"]
    print(vc["time.idle"].count())
    print("view mean")
    mean = vc["time.idle"].mean()
    print(mean.isoformat())
    std = vc["time.idle"].std()
    print(std.isoformat())
    vc["time.idle"].plot()


def make_table(file: str, tag: int) -> pd.DataFrame:
    log = load_log(file)
    log["source"] = tag
    return log


# %%
file0 = "../logs/mcom1.log"
log0 = make_table(file0, 0)

file1 = "../logs/mcom2.log"
log1 = make_table(file1, 1)

file2 = "../logs/mcom3.log"
log2 = make_table(file2, 2)

file3 = "../logs/mcom4.log"
log3 = make_table(file3, 3)


# %%

def request_latency(table: pd.DataFrame):
    requests = timedelta_series(
            table, "time.idle", "target", "__measure::request")
    requests["time.idle"].plot()
    print(requests["time.idle"].count())
    mean = requests["time.idle"].mean()
    print("request mean")
    print(mean.isoformat())
    print("request std")
    print(requests["time.idle"].std().isoformat())


request_latency(log0)
request_latency(log1)
request_latency(log2)
request_latency(log3)


# %%

def request_rate(table: pd.DataFrame):
    reqs = table[table["target"] == "__measure::app"]
    reqs = reqs[reqs["message"] == "execute"]
    rps = reqs["message"].resample("5S").count()
    rps.plot()


request_rate(log0)
request_rate(log1)
request_rate(log2)
request_rate(log3)

# %%
view_changes(log0)
view_changes(log1)
view_changes(log2)
view_changes(log3)
