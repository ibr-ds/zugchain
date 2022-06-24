#!/bin/bash

nohup ./railchain --config './config/*' $1 2>&1 >railchain.log </dev/null &
echo $! > railchain.pid