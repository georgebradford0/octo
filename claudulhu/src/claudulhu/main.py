import argparse


def main():
    parser = argparse.ArgumentParser(prog="claudulhu")
    subparsers = parser.add_subparsers(dest="command")

    task_parser = subparsers.add_parser("task", help="Run a task")
    task_parser.add_argument("description", help="Task description")

    args = parser.parse_args()
    if args.command == "task":
        print(args.description)


if __name__ == "__main__":
    main()
