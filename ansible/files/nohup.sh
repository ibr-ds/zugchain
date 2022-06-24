#!/bin/bash

program=$1
shift
config $1
shift

nohup ./$program --config $config $@ 2>&1 >$program.log </dev/null &
echo $! > $program.pid
sleep 1;