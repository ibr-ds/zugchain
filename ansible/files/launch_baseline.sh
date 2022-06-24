#!/bin/bash

nohup ./baseline-server --config './config-baseline/*' $1 2>&1 >baseline-server.log </dev/null &
echo $! > baseline-server.pid

sleep 2

nohup ./baseline-client --config './config-baseline/*' $1 2>&1 >baseline-client.log </dev/null &
echo $! > baseline-client.pid