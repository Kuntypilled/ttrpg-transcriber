#!/usr/bin/env bash

# Define the number of windows per column and the target number of columns
# Example: 8 windows total will result in 4 columns of 2 windows each
WINDOWS_PER_COLUMN=2
TOTAL_COLUMNS=4

# Start the process from the leftmost column
niri msg action focus-column-first
sleep 0.1

for ((c=0; c<TOTAL_COLUMNS; c++)); do
    # Set the current column width to 50% of the screen
    niri msg action set-column-width "33.333333%"
    sleep 0.05

    # Pull the neighboring window into the current column (creating a pair)
    niri msg action consume-window-into-column
    sleep 0.05

    # Move focus to the next available column on the right
    niri msg action focus-column-right
    sleep 0.1
done

# Return focus to the first column to finish the sequence
niri msg action focus-column-first
