#!/bin/bash

# Generated run script for cs-studio.

SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
TOP=${SCRIPT_DIR}/../../..

# The location of css.sh
CSS_RUN_SCRIPT=$(configure-ioc s -p CSS-gui)


# pop the opifile off the head of the argument list
opifile="$1"
shift

function usage() {
    printf "\\nUsage: $0 opi-file -m macro1=value1,macro2=value2\\n\\n"
    printf "       opi-file path should be relative to $(basename "$0")\\n\\n"
    printf "       Additional arguments are passed to css.sh.\\n"
    printf "       See $CSS_RUN_SCRIPT -h for more.\\n\\n"
}

# Check for valid arguments.
if [[ -z $opifile || $opifile == "-h" || $opifile == "--help" ]]; then
    usage
    exit 1
fi
# Check that requested opi file exists.
if [[ ! -f $SCRIPT_DIR/$opifile ]]; then
    echo "OPI file $SCRIPT_DIR/$opifile does not exist"
    usage
    exit 1
fi

if [ -e "${TOP}/configure/VERSION" ] ; then
    version="$(cat "${TOP}"/configure/VERSION)"
else
    version="dev"
fi
project=vacuum_${version}
module=vacuum
launch_opi=/${project}/${module}/$opifile

links="${SCRIPT_DIR}=${project}/${module},\
/dls_sw/prod/R3.14.12.7/support/dlsPLC/3-0-68/dlsPLCApp/opi/opi=/${project}/dlsPLC,\
/dls_sw/prod/R3.14.12.7/support/mks937a/2-90/mks937aApp/opi/opi=/${project}/mks937a,\
/dls_sw/prod/R3.14.12.7/support/mks937b/2-94/mks937bApp/opi/opi=/${project}/mks937b,\
/dls_sw/prod/R3.14.12.7/support/saesNegPower/1-0/saesNegPowerApp/opi/opi=/${project}/saesNegPower,\
/dls_sw/prod/R3.14.12.7/support/QPC/2-3/digitelQpcApp/opi/opi=/${project}/QPC,\
/dls_sw/prod/R3.14.12.7/support/enzLoCuM4/2-39/enzLoCuM4App/opi/opi=/${project}/enzLoCuM4,\
/dls_sw/prod/R3.14.12.7/support/terminalServer/2-6/terminalServerApp/opi/opi=/${project}/terminalServer,\
/dls_sw/prod/R3.14.12.7/support/rga/4-21/rgaApp/opi/opi=/${project}/rga,\
/dls_sw/prod/R3.14.12.7/support/vacuumSpace/4-6/vacuumSpaceApp/opi/opi=/${project}/vacuumSpace,\
/dls_sw/prod/R3.14.12.7/support/digitelMpc/6-23/digitelMpcApp/opi/opi=/${project}/digitelMpc,\
/dls_sw/prod/R3.14.12.7/support/rackFan/2-12/rackFanApp/opi/opi=/${project}/rackFan,\
/dls_sw/prod/R3.14.12.7/support/procServControl/1-22/procServControlApp/opi/opi=/${project}/procServControl,\
/dls_sw/prod/R3.14.12.7/support/devIocStats/3-1-14dls3-2/devIocStatsApp/opi/opi=/${project}/devIocStats"

$CSS_RUN_SCRIPT -o "${launch_opi}" -s -l "$links" "$@"
