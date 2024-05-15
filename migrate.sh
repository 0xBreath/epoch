#!/bin/bash


down_file="timescale/migrations/epoch.down.sql"
down_suffix=".down.sql"

for target_file in "migrations/"*"$down_suffix"; do
  if [ "$down_file" != "$target_file" ]; then
    cp "$down_file" "$target_file"
    echo "Copied $down_file to $target_file"
  fi
done

up_file="timescale/migrations/epoch.up.sql"
up_suffix=".up.sql"

for target_file in "migrations/"*"$up_suffix"; do
  if [ "$up_file" != "$target_file" ]; then
    cp "$up_file" "$target_file"
    echo "Copied $up_file to $target_file"
  fi
done