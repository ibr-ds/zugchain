#!/bin/bash

pidfile="$1.pid"

if [[ -r $pidfile ]]; then
    kill $(<$pidfile);
fi

rm -f $pidfile