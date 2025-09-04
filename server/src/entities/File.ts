import { Entity, Column, PrimaryColumn, ManyToOne, JoinColumn } from "typeorm";
import { User } from "./User";
import { Group } from "./Group";

const BigIntTransformer = {
  to:   (v: bigint | null) => v == null ? null : v.toString(), // -> DB
  from: (v: string | null) => v == null ? null : BigInt(v),    // <- DB
};

@Entity()
export class File {
  @PrimaryColumn({type:"bigint", transformer: BigIntTransformer})
  ino: bigint;

  @Column({nullable:false})
  path:string;

  @ManyToOne(() => User, (user) => user.files)
  @JoinColumn({ name: "owner", referencedColumnName: "uid" })
  owner: User;

  @Column({nullable:false})
  type: number; // 0 = file, 1 = directory, 2 = symlink, etc.

  @Column({nullable:false})
  permissions: number;

  @ManyToOne(() => Group, (group) => group.gid, { nullable: true })
  @JoinColumn({ name: "group" })
  group: Group;

}