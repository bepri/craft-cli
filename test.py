from craft_cli.printer import Mode, Printer
from time import sleep

printer = Printer(Mode.VERBOSE)

printer.progress("starting", permanent=False)
sleep(5)
printer.progress("processing", permanent=False)
sleep(2)
printer.progress("end", permanent=True)
