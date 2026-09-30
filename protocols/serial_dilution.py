from opentrons import protocol_api

metadata = {
    "protocolName": "Serial Dilution",
    "description": "Short OT-2 serial dilution used by a2a-lab-sdk-example.",
    "author": "A3 Analytics",
}

requirements = {"robotType": "OT-2", "apiLevel": "2.16"}


def run(protocol: protocol_api.ProtocolContext) -> None:
    tips = protocol.load_labware("opentrons_96_tiprack_300ul", 1)
    reservoir = protocol.load_labware("nest_12_reservoir_15ml", 2)
    plate = protocol.load_labware("nest_96_wellplate_200ul_flat", 3)
    pipette = protocol.load_instrument("p300_single", "right", tip_racks=[tips])

    diluent = reservoir["A1"]
    stock = reservoir["A2"]
    row = plate.rows()[0][:3]

    protocol.pause("waiting to start serial dilution")
    pipette.transfer(100, diluent, row)
    protocol.pause("diluent dispensed")
    pipette.transfer(50, stock, row[0], mix_after=(3, 40))
    protocol.pause("stock mixed")
    pipette.transfer(50, row[0], row[1], mix_after=(3, 40))
    pipette.transfer(50, row[1], row[2], mix_after=(3, 40))
